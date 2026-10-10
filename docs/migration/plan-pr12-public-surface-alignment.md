# Plan PR12 — Public Surface Alignment

Status: **READY FOR REVIEW — implementation and focused validation complete**.

```text
# CHECKPOINT PR11 MERGE

PR: #14 — Plan PR11: retire Main Agent, Project Agent, and Project OS verticals
HEAD approved: f6055ce33a1a45fb6ab0786399d7cfc3455ee29a
merge SHA: 87eb2a4f5a96fc45b44e72d02b642c835285cf8b
origin/main: 87eb2a4f5a96fc45b44e72d02b642c835285cf8b
ancestry: PASS — approved HEAD is an ancestor of origin/main
working tree: clean at phase transition
result: PASS
```

## Baseline

- Post-PR11 `origin/main`: `87eb2a4f5a96fc45b44e72d02b642c835285cf8b`.
- Branch: `feat/plan-pr12-public-surface-alignment`.
- Merge base: `87eb2a4f5a96fc45b44e72d02b642c835285cf8b`.
- Migration head: V120 (`V120__preserve_interactive_task_role_lease_authority.sql`).
- Working tree at inventory start: clean.
- PR11 merge: #14, approved HEAD `f6055ce33a1a45fb6ab0786399d7cfc3455ee29a`,
  normal merge `87eb2a4f5a96fc45b44e72d02b642c835285cf8b`.

## Scope and authority

The public contract describes Project, Repo, Task, Actor, Agent, TaskRole,
RoleMembership, Execution, HarnessSession, WorkUnit, Workspace, WorkspaceLease,
Message, Handoff, Proposal, Decision, Artifact, Review Execution, ReviewReport,
ValidationRun, Evidence, Gate, TaskLifecycle, and the generic Orchestrator.
The orchestrator owns work; the harness owns cognition. No legacy projection
may choose an Actor, Role, HarnessSession, Execution, or provenance record.

This PR owns REST, MCP, `forge-ctl`, web/UI, exported/generated public types,
and the public SSE vocabulary. It does not drop schema, edit historical
migrations, rename Forge, or create cross-model synchronization.

## CHECKPOINT PR12 INVENTORY

### Surface map

| Surface | Current implementation found | Initial classification and decision |
| --- | --- | --- |
| REST | `crates/api/src/lib.rs` registers account/Main Agent and Genesis, memory/context, baseline, Charter, Project Documents/Decisions, milestones/readiness/releases, Project overview, Project Agent/Chat/handoff, commitments/inbox/questions/actions, workflow/templates, legacy Task actions/transition/review, native AgentSession, plus target Task/Gate/collaboration/Execution/provider routes. | Remove obsolete registrations. Keep target Project/Repo/Task/role/collaboration/Execution/Gate/Review/Validation/Evidence/provider routes. Keep only immutable Release snapshot GETs as `HISTORICAL_READ_ONLY`. Retire the PR11 `operation_retired` REST path fence after obsolete routes are gone. |
| MCP | `tools/descriptors.rs`, `tools/handlers.rs`, `tools/mod.rs`, `rpc.rs`; descriptors include Task CRUD/status transition, memory, singular Agent assignment, Projects/hooks, provider profiles and AgentSessions, Main/Project bindings, Agent Chat and old handoffs. `rpc.rs` has an unannounced retired-tool compatibility dispatcher. | Remove retired descriptors, params, handlers, and the compatibility dispatcher. Keep useful provider/profile and target Task/Project operations. Replace Task status/assignment with TaskLifecycle and TaskRole membership. Add the smallest useful target Gate, Review, ValidationRun, Evidence, and generic collaboration tools after their REST handlers are verified. |
| `forge-ctl` | `main.rs` exposes `memory` and umbrella `embedded`; `embedded.rs` combines dead Main/Project Agent, Chat, old Handoff, native Agent/session/context/commitment commands with still-useful provider entries and Agent profile operations. `task.rs` filters `--status` and writes `/transition`. | Remove Memory and Embedded commands. Move surviving provider operations to `provider` and profile operations to `agent`; delete dead vertical commands. Replace Task status display/filter/mutation with lifecycle state and exact lifecycle version. Keep server `forge-cli` untouched. |
| Web/UI | `router.tsx`, `app-shell.tsx`, `ChatPage`, `MissionControlPage`, `ProjectOverviewPage`, `features/agent-chat`, Product Genesis controls, Main/Project binding settings, Workflow settings and legacy task/board/detail status logic. Project creation still carries Project Agent IDs and setup copy. | Remove operational Chat, Mission Control/Attention, Genesis, binding, and workflow surfaces. Make Project creation ordinary. Keep Task/board and project hooks; move them to TaskLifecycle/Gate. Replace Project OS overview with a small target Project/Task view if the current route still serves as the primary Project entry. Keep a read-only Release snapshot route only if its fetch path remains immutable and does not recalculate readiness. |
| Generated/public types | `api-types::export_typescript()` exports target types together with Agent Chat, Main/Project Agent, memory, Attention, Charter, baseline, Project Documents/Decisions, milestones/readiness, old workflow, singular role assignment, and legacy Task status DTOs. Generated bindings and `web/src/types/generated` contain these exports. | Change Rust/API source first; retain internal/storage Rust models where needed. Remove dead items from the TypeScript export set, regenerate after web/API contracts settle, then delete only stale generated bindings and verify no orphan imports. |
| SSE/EventBus | `routes/events.rs` serializes internal `ForgeEvent` directly. `EventContext` contains legacy TaskStatus/recovery/assignment, old Review/workflow effects, Agent Chat progress, and target event contexts. The web invalidation client consumes legacy status and Agent Chat names. Durable DomainEvents are separate. | Keep internal signals needed by PR13-era code. Add one bounded explicit SSE projection/allowlist; emit target lifecycle and exact Gate/fact vocabulary only, redact payloads, preserve `events.resync_required`, and never make EventBus authoritative. |
| TaskLifecycle/Gate | REST already has GET lifecycle, Gate policy/evaluation, and merge-after-Gate. It also registers `/tasks/{id}/transition`, `/transitions`, Gate approve/reject compatibility routes, and Task response/list fields derived from legacy `task.status`. MCP/CLI/web still expose status/workflow paths. | Remove status/workflow projections. Add exact TaskLifecycle mutation/transition receipt projection where the existing service and receipt model permit it. Filter/list/order by TaskLifecycle. Keep Gate evaluation inputs and policy revision exact; do not infer readiness from workflow or stale evaluations. |
| Review/Validation/Evidence | REST has target Review Execution/report, ValidationRun, and Evidence handlers; MCP lacks the complete target set. Web has a Task Review surface and legacy review history/task actions to audit. | Keep formal Review only for `Execution.role=reviewer AND purpose=review`; expose frozen subject/report and exact validation/evidence identities. Remove legacy Review-row authority and any latest-review inference. Add only useful MCP operations over exact IDs. |
| Historical reads | ProjectRelease snapshot endpoints/page can render immutable release data with pinned assets/evidence. PR11 migration disposition rows and old vertical records remain in SQLite. | Retain immutable Release GETs as `HISTORICAL_READ_ONLY` if confirmed no recomputation or mutation. Do not retain Main/Project Chat, Attention, readiness, Charter, baseline, or legacy Review reads as current representations. PR11 audit/history remains available in DB and migration ledger until PR13. |
| PR13 storage | V001–V120 schema includes `task.status`, singular role assignment, workflow/GateConfig, old Review, Chat/AgentSession, Main/Project Agent, Genesis, memory, Attention, Charter/baseline, Project documents/decisions, milestones/readiness/releases, and compatibility readers. | `PR13_STORAGE_ONLY` when no public consumer remains. Preserve DB fences and all historical rows; no migration SQL or schema drop in this PR. |
| PR14 naming | Forge product name, `forge-ctl`, `forge-cli`, config/data paths and package/crate names. | `PR14_DOC_OR_BRANDING`; no renames in this PR. |

### Required compatibility ledger

| Legacy surface | Old writer / new writer | Old reader / new reader | Authority during transition | Dual write and divergence protection | Cleanup / data and rollback |
| --- | --- | --- | --- | --- | --- |
| `task.status`, legacy Task transition | Old TaskService transition and status request / TaskLifecycleService exact transition request. | Old REST/MCP/CLI/UI status / lifecycle response, filter, receipt, and Gate. | `task_lifecycle` plus immutable transition receipt; `task.status` remains only its guarded one-way DB projection. | `NONE`; V100+ DB invariant protects the projection. Public readers stop consulting it. | Public cleanup PR12; column/projection cleanup PR13. Preserve status and transition history; rollback may read DB but old writers remain fenced. |
| Singular `task_role_assignment` and `role_assignments` DTO | Old singular writer / TaskRole + RoleMembership writer. | Old assignee response / TaskRole memberships with exact Actor and membership state. | TaskRole + RoleMembership; never choose the compatibility projection. | `NONE` in public contracts; PR11/PR13 storage compatibility remains fenced and bounded. | Public fields PR12; physical cleanup PR13. Preserve rows and do not synthesize replacement memberships. |
| Main/Project Agent, Genesis, Agent Chat, Attention, semantic memory, commitments/questions/inbox/actions, Charter, baseline, Documents/Decisions, milestones/readiness | PR11 writers already retired / no replacement writer except ordinary Project and Task-scoped generic primitives for newly initiated work. | Legacy operational readers / no current reader; exact generic records are read only when actually created with exact provenance. | No legacy authority; Project/Task and generic records alone govern new work. | `NONE`; do not translate, alias, infer, or dual-write. | Public removal PR12; storage cleanup PR13. Preserve historical rows unchanged; rollback never restores their writer or dispatch authority. |
| Project Release | Legacy release writer already retired / no new writer. | Mutable Project OS/readiness projection / immutable stored snapshot GET only. | Snapshot and exact pinned references are historical evidence, never current Gate authority. | `NONE`; no recalculation or new Release. | Historical GET PR12; storage decision PR13. Preserve rows, media pins and bytes. |
| Embedded/native AgentSession | Native runtime/session writer retired by PR10/PR11 / none. | Operational session controls / none; provider credentials and external Agent profiles remain separate target surfaces. | No current runtime authority. Harness identity/profile and explicit HarnessSession remain their own domain records. | `NONE`; no legacy session to HarnessSession conversion or fallback. | Public controls PR12; encrypted/history storage PR13. Preserve encrypted history without revival. |
| Legacy workflow/configuration and GateConfig | Old workflow and state-driven hook writers / TaskLifecycle and Gate writers for their distinct semantics; generic Project hooks have their own writer. | Workflow state/config, EventBus-driven state scripts and transition logs / exact TaskLifecycle, GatePolicyRevision, GateEvaluation and receipts; durable generic Project hooks consume their own exact DomainEvent facts. | TaskLifecycle + Gate; legacy scripts no longer dispatch or block Executions. Generic Project hooks remain independent. | `NONE`; never convert workflow definitions or GateConfig into Gate policies, or EventBus hints into authority. | Legacy script dispatch removal PR12; engine/storage PR13. Preserve historical definitions, settings and logs. |
| `ProjectSettings.default_role_assignments` | Old Project settings writer / explicit TaskRole and RoleMembership creation only. | Old Task creation reader seeded RoleMemberships / no implicit settings reader. Exact imported-issue defaults remain only when a current integration explicitly names an ActorRef. | TaskRole + RoleMembership created from current explicit input; historical Project settings have no membership authority. | `NONE`; no translation from old assignment fields and no inferred Actor. | Reader/public contract PR12; stored setting cleanup PR13. Preserve the exact settings JSON; new Tasks do not inherit it. |
| Issue integration defaults and credential reference | Old public writer accepted `default_task_state`, singular `default_assignee_*`, and `token_secret_ref` (an environment-variable name); sync could seed a singleton coder assignment. / New writer accepts `credential_env_var` and optional exact `default_implementer: ActorRef`; imported issues create ordinary Tasks and use TaskRole/RoleMembership for the configured implementer. | Old reader returned the internal reference name and legacy defaults. / New reader returns `credential_env_var` and the exact ActorRef when configured. The environment variable's secret value is never returned or stored. | Exact ProjectIntegration row controls external issue fetch; TaskLifecycle begins at normal Task creation; a validated Actor is added to the imported Task's `implementer` RoleMembership. | `NONE`; no dual write. Existing legacy storage columns preserve the exact env-var key and ActorRef representation until PR13; `default_task_state` is ignored. Invalid or malformed Actor config fails closed before import. | Public DTO and consumer PR12; physical columns PR13. Preserve the integration row and the existing env-var key; rollback can read the existing row but does not change it. |
| PR provider configuration | Create-only writer and UI PATCH fields that were ignored. / Create/PATCH now write `pr_provider_config` explicitly. | `repo_response` hid stored configuration. / Repo GET/list/create/PATCH report provider type, token presence, and poll interval without secret. | Exact `pr_provider_config` row; no latest provider/config inference. | `NONE`; omitted token preserves the current value, explicit null clears it, and direct-merge UI clears provider configuration. | Public contract PR12; no physical cleanup currently scheduled. No schema change; stored credentials are preserved unless explicitly cleared. |
| `public_search` server configuration and `FORGE_PUBLIC_SEARCH_*` environment variables | Old chat-only config writer / no replacement writer or built-in search capability. | Old Agent Chat tool discovery / no public consumer. External Harnesses own their research capabilities. | No Forge search authority or stored search data. | `NONE`; stale YAML is rejected as an unknown top-level field. Removed environment variables are no longer read. | Remove config surface PR12; no stored data or migration. Rollback requires removing the stale setting and is not an operational fallback. |
| Operator status Task and retry projection | Old public reader used `task.status`, `transition_log`, legacy retry metadata, and `execution.agent_session_id` / new reader uses TaskLifecycle plus exact transition/retry receipt facts and `execution.harness_session_id`. | Old Operations page displayed workflow state, guessed retry counts, and legacy session ID / new page renders lifecycle state/version, exact receipt IDs, and explicit HarnessSession identity. | TaskLifecycle and its current transition receipt; immutable `task_failure_retry_receipt` plus exact DomainEvent links; explicit Execution HarnessSession reference. | `NONE`; old status/log/metadata do not feed the new projection and legacy AgentSession is never a HarnessSession fallback. | Public contract/UI PR12; legacy columns/logs remain PR13 storage. Preserve old rows unchanged; rollback can inspect storage but cannot revive them as operational truth. |

### Final static census classifications

- `TARGET_V2_KEEP`: REST/MCP/CLI/web Project, Repo, Task, Actor, Agent,
  TaskRole/RoleMembership, Execution/HarnessSession, WorkUnit/Workspace/Lease,
  generic Message/Handoff/Proposal/Decision/Artifact, exact Review Execution and
  ReviewReport, ValidationRun/Evidence, Gate/GatePolicyRevision/GateEvaluation,
  TaskLifecycle, generic Project hooks, provider credentials/profiles, OAuth,
  integrations, media, auth, notifications, and operations status. WorkUnit
  allocation readiness is derived from exact WorkUnit/dependency/Execution facts.
- `REPLACE_WITH_V2`: Task responses, list filters, ordering, board/detail and
  CLI lifecycle controls now read TaskLifecycle and exact versions/receipts;
  merge readiness uses exact GateEvaluation facts; review UI and MCP use exact
  ReviewReport/ValidationRun/Evidence identities; SSE maps durable DomainEvents
  to a typed allowlist; operator health uses current TaskLifecycle and exact
  retry receipts, and exposes HarnessSession only from its exact Execution
  reference. No `task.status` or legacy `agent_session_id` public reader remains.
- `HISTORICAL_READ_ONLY`: exact ProjectRelease GET/page and pinned evidence/media
  only. The page labels its immutable snapshot historical and states it does not
  represent current TaskLifecycle, Gate, or Project authority. It does not
  recompute readiness and offers no mutation.
- `REMOVE_PUBLIC`: Main/Project Agent operations, Product Genesis, Agent Chat,
  embedded/native Agent runtime and session controls, AgentAction, commitments,
  questions, inbox, semantic memory/context manifests, Attention/Mission Control,
  Charter/baseline/Project documents and decisions, milestone/readiness/release
  writers, workflow/status APIs and UI, legacy Review rows/actions, retired Task
  transition/action routes, PR11 410 compatibility dispatchers, Memory backfill,
  the `embedded` CLI umbrella, and the unused public search config/tool surface.
- `PR13_PHYSICAL_CLEANUP`: persisted `task.status` projection and migration
  audit, singular role storage, workflow definitions/GateConfig/transition logs,
  legacy Review/Chat/AgentSession/Genesis/Project OS records and encrypted
  history, retired EventContext variants, dormant workflow/recovery modules,
  lifecycle hook script config/runner, `ProjectSettings.default_role_assignments`,
  and integration compatibility columns. No SQL migration or `DROP` is present.
- `MIGRATION`: V001–V120 and V118/V120 dispositions remain immutable; the
  migration ledger and old row provenance remain intact. No current writer,
  dispatch, or authority is restored from those records.
- `TEST_FIXTURE`: negative contract assertions mention retired REST fields,
  event names, MCP names, and `public_search` only to prove rejection or
  suppression; migration fixtures preserve their exact historical inputs.
- `DOCUMENTATION_HISTORY`: prior Plan PR documents, V071–V076 migration notes,
  `docs/concepts/agents-and-harnesses.md` transition history,
  `docs/architecture-review-task-actor.md`, and legacy references in old
  Architecture V2 checkpoints. `getting-started.md` explicitly marks its
  V071–V076 material historical.
- `PR14_DOC_OR_BRANDING`: Forge, `forge-ctl`, `forge-cli`, package/crate, config,
  and data-directory naming remain unchanged.
- `BUG_STILL_LIVE`: 0 after the final route, consumer, generated-type, and event
  census; no residual legacy public authority or silent translation remains.

## Implementation checkpoints

- **Checkpoint 0 — PR11 merge: PASS.** PR #14 merged with a normal merge
  commit. Approved HEAD `f6055ce33a1a45fb6ab0786399d7cfc3455ee29a` is an
  ancestor of post-merge `origin/main` `87eb2a4f5a96fc45b44e72d02b642c835285cf8b`;
  the PR branch/base and clean post-merge tree were verified before creating
  this branch.
- **Checkpoint 1 — inventory: PASS.** The full REST router, MCP descriptors and
  dispatcher, `forge-ctl`, UI routes/navigation/hooks, generated export set,
  API request/response DTOs, all 82 EventContext variants, and SSE consumers
  were inspected at the V120 base. The surface map and event matrix above are
  the recorded inventory.
- **Checkpoint 2 — REST/API: PASS.** Removed the
  PR11 obsolete-route registrations and temporary REST retirement fence;
  Task responses/filters use TaskLifecycle; exact Gate, Review Execution,
  ReviewReport, ValidationRun, Evidence, TaskRole, and RoleMembership routes
  remain public. Repo provider configuration now has a working PATCH/read
  contract; issue-integration DTOs no longer expose secrets or legacy task
  state/singular assignment, while exact `default_implementer` config creates
  a validated implementer TaskRole membership on imported Tasks. Operator
  status reads TaskLifecycle and exact retry receipts and reports only explicit
  HarnessSession references.
- **Checkpoint 3 — MCP: PASS.** Removed retired
  descriptors, params, handlers, and compatibility dispatcher. Target tools
  expose lifecycle, Gate, roles/membership, Review, validation/Evidence, and
  generic collaboration by exact identities.
- **Checkpoint 4 — `forge-ctl`: PASS.** Removed
  retired `embedded` and memory-backfill commands; provider/profile functions
  remain under their target namespaces. Task list and transition use lifecycle
  state/version.
- **Checkpoint 5 — Web/UI: PASS.** Removed operational
  Main/Project Chat, Mission Control, Genesis/setup, binding, workflow, and
  legacy review controls. Project creation is ordinary. Generic Project Hooks
  remain. Stale executable E2E specs that asserted retired verticals or
  workflow/status APIs were removed; target page/terminal checks remain.
- **Checkpoint 6 — generated types: PASS.** Rust
  source drives one `ts-rs` generation pass after contract stabilization. The
  explicit export barrel and dependency closure contain only used target DTOs
  plus immutable Release history; obsolete request/response exports and orphan
  bindings were removed.
- **Checkpoint 7 — public events/SSE: PASS.** An
  explicit typed public projection maps exact durable DomainEvents and a small
  runtime-hint allowlist. Legacy status, workflow, assignment, Recovery,
  legacy Review, and Agent Chat signals do not escape through SSE.
- **Checkpoint 8 — final census and focused regression: PASS.** No live legacy
  public authority or compatibility dispatcher remains. Focused API, service,
  DB-retirement, MCP, CLI, web, generated type, lifecycle/Gate, Review, and SSE
  checks pass.
- **Checkpoint 9 — architecture/docs/publication: implementation PASS.**
  Architecture V2, architecture, API, CLI, getting-started, changelog, and this
  ledger describe the target contract. The branch is published for human review;
  PR12 is not merged.

## Validation record

Focused checks passed:

- `cargo test -p api --test pr12_public_surface --locked --offline` (7 tests)
- `cargo test -p api --test operations_status --locked --offline` (4 tests)
- `cargo test -p api --test happy_path --locked --offline` (2 tests)
- `cargo test -p api --lib routes::events:: --locked --offline` (4 tests)
- `cargo test -p api --test project_hooks_public_surface --locked --offline` (1 test)
- `cargo test -p services --lib operator_status::tests:: --locked --offline` (11 tests)
- Focused service tests for ordinary Task role creation, legacy hook dispatch,
  Execution dispatch, formal Review Execution, and retired recovery actions
- `cargo test -p db --test pr11_retirement v118_ --locked --offline` (history and
  database retirement fences, 2 tests)
- `cargo test -p mcp-server --locked --offline` (37 tests)
- `cargo test -p forge-client --test integration --locked --offline` (4 tests)
- `CARGO_NET_OFFLINE=true pnpm --dir web generate:types` (one Rust export test)
- `pnpm --dir web typecheck`
- Focused Vitest for `OperationsPage` (7 tests) and `BoardPage` lifecycle
  authority (1 test); earlier focused web regressions passed 19 tests across
  five affected files.
- Focused config test rejecting the removed `public_search` setting
- `FORGE_SKIP_WEB_BUILD=1 cargo check -p forge-cli --locked --offline`
- `cargo fmt --all -- --check`, `git diff --check`, and focused `forge-ctl` help
  checks for the public command tree, lifecycle list, and transition contract.

Not run: workspace-wide Cargo tests/build/clippy, the full web Vitest suite, and
Playwright E2E. This change used focused crates and page tests; no local server
E2E run was needed for the contracts above. No SQL migration, `cargo clean`, or
workspace-wide build was run. The reused `target/` measured about 35 GB and the
filesystem retained about 72 GB free after scoped validation.

## Public EventContext inventory and SSE projection matrix

The 82 `EventContext` variants below were scanned in production Rust source.
Producer paths and string event names come from their `ForgeEvent` constructors;
`none found` means there is no non-test constructor in the current tree. EventBus
contexts are notifications, never authority. SSE resolves `domain_event.committed`
by exact event ID and projects only the durable event allowlist; it never
serializes the internal context. The public lifecycle and Gate event vocabulary
is DomainEvent-only (`task.lifecycle_changed`, `gate.created`,
`gate.policy_revised`, `gate.evaluated`) and has no legacy EventContext variant.

| Variant | Observed event_type / producer(s) | Internal consumer(s) | Web/SSE | Durable DomainEvent equivalent | Authority / target classification / decision |
| --- | --- | --- | --- | --- | --- |
| `TaskCreated` | task.created".to_owned() @ crates/services/src/task_service/create.rs; task.created".to_owned() @ crates/services/src/task_service/create_subtasks.rs | ProjectHookService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskStatusChanged` | task.status_changed".to_owned() @ crates/services/src/task_service/claim.rs; task.status_changed".to_owned() @ crates/services/src/task_service/transition.rs | ProjectHookService, NotificationService, OperatorStatusEmitter, legacy lifecycle emitter | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskMoved` | TASK_MOVED_EVENT.to_owned() @ crates/services/src/task_service/move_task.rs | ProjectHookService, NotificationService, legacy lifecycle emitter | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskAutoTransitioned` | task.auto_transitioned".to_owned() @ crates/services/src/task_service/execution/cascade.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskAssigned` | task.assigned".to_owned() @ crates/services/src/task_service/claim.rs; task.execution_launched".to_owned() @ crates/services/src/task_service/execution/launch.rs | legacy lifecycle emitter | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskRoleReassigned` | task.role_reassigned".to_owned() @ crates/services/src/task_service/roles.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskBlocked` | task.blocked".to_owned() @ crates/services/src/task_service/execution/cascade.rs; task.blocked".to_owned() @ crates/services/src/task_service/execution/runner.rs; task.blocked".to_string() @ crates/services/src/workflow/actions/common.rs; task.blocked".to_string() @ crates/services/src/workflow/actions/lifecycle.rs | NotificationService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskUnblocked` | task.unblocked".to_owned() @ crates/services/src/task_service/execution/recovery.rs; task.unblocked".to_owned() @ crates/services/src/task_service/move_task.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskFailed` | task.failed".to_owned() @ crates/services/src/task_service/execution/recovery.rs | NotificationService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskRestarted` | task.restarted".to_owned() @ crates/services/src/task_service/execution/recovery.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskCancelled` | task.cancelled".to_string() @ crates/services/src/workflow/actions/lifecycle.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskRecovered` | task.execution_resumed".to_owned() @ crates/services/src/task_service/execution/recovery.rs; task.recovered".to_owned() @ crates/services/src/recovery.rs; task.recovered".to_owned() @ crates/services/src/shutdown.rs; task.recovery_action".to_owned() @ crates/services/src/task_service/execution/recovery.rs | NotificationService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `RecoveryApplied` | task.recovery_applied".to_owned() @ crates/services/src/task_service/execution/recovery.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskUpdated` | task.archived".to_owned() @ crates/services/src/task_service/transition.rs; task.updated".to_owned() @ crates/services/src/project_actor_scope.rs; task.updated".to_owned() @ crates/services/src/task_service/claim.rs; task.updated".to_owned() @ crates/services/src/task_service/memberships.rs; task.updated".to_owned() @ crates/services/src/task_service/reorder_subtasks.rs | ProjectHookService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskDeleted` | task.deleted".to_owned() @ crates/services/src/task_service/transition.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskDependencySatisfied` | task.dependency_satisfied".to_string() @ crates/services/src/workflow/actions/subtasks.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ExecutionStarted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `execution.started` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ExecutionLog` | execution.log".to_owned() @ crates/services/src/daemon_transport/execution_events.rs; execution.log".to_owned() @ crates/services/src/task_service/execution/runner.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ExecutionCompleted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `execution.completed` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ExecutionFailed` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `execution.failed` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ExecutionCancelled` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `execution.cancelled` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskExecutionRetry` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ExecutionStalled` | execution.stalled".to_owned() @ crates/services/src/recovery.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `execution.stalled` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ExecutionDaemonDisconnected` | execution.daemon_disconnected".to_owned() @ crates/services/src/recovery.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ReconciliationEvent` | operations.refreshed".to_owned() @ crates/api/src/routes/operations.rs; reconciliation.event".to_owned() @ crates/services/src/daemon_monitor.rs; reconciliation.event".to_owned() @ crates/services/src/daemon_service.rs; reconciliation.event".to_owned() @ crates/services/src/recovery.rs; reconciliation.event".to_owned() @ crates/services/src/task_service/roles.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskExecutionCancelled` | task.execution_cancelled".to_owned() @ crates/services/src/task_service/roles.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `AgentStatusChanged` | agent.status_changed".to_owned() @ crates/services/src/agent_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `AgentCreated` | agent.created".to_owned() @ crates/services/src/agent_service.rs; agent.created".to_owned() @ crates/services/src/daemon_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `AgentArchived` | agent.archived".to_owned() @ crates/services/src/agent_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `AgentPaused` | agent.paused".to_owned() @ crates/api/src/routes/agents.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `AgentResumed` | agent.resumed".to_owned() @ crates/api/src/routes/agents.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ProfileCreated` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ProfileUpdated` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ProfileDeleted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `AgentTimeout` | agent.timeout".to_owned() @ crates/services/src/recovery.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `WorkspaceCreated` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `WorkspaceExecutionWaiting` | workspace.execution_waiting".to_owned() @ crates/services/src/task_service/execution/runner.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `WorkspaceCleaned` | workspace.cleaned".to_owned() @ crates/services/src/workspace_cleanup.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskTerminalSessionChanged` | TERMINAL_SESSION_CHANGED_EVENT.to_owned() @ crates/services/src/terminal_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskSubtaskSequenceStarted` | task.subtask_sequence_started".to_owned() @ crates/services/src/task_service/execution/subtasks/mod.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskSubtaskSequencePaused` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskSubtaskSequenceResumed` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskSubtaskCommitRecorded` | task.subtask_commit_recorded".to_owned() @ crates/services/src/task_service/execution/subtasks/mod.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ReviewStarted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ReviewDecided` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ReviewPassed` | none found in non-test producer scan | NotificationService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ReviewFailed` | none found in non-test producer scan | NotificationService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ReviewApproved` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ReviewRejected` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `MergeStarted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `MergeSucceeded` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `MergeFailed` | merge.failed".to_string() @ crates/services/src/workflow/actions/merge.rs | NotificationService | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `CommentCreated` | comment.created".to_owned() @ crates/services/src/task_service/common.rs; comment.created".to_string() @ crates/services/src/workflow/actions/common.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskMediaUploaded` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskMediaDeleted` | task.media.deleted".to_owned() @ crates/api/src/routes/tasks/media.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `FollowUpDispatched` | follow_up.dispatched".to_owned() @ crates/services/src/task_service/execution/cascade.rs; follow_up.dispatched".to_owned() @ crates/services/src/task_service/execution/follow_up.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TaskRoleAgentDispatched` | task.role_agent_dispatched".to_string() @ crates/services/src/workflow/actions/dispatch.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskAwaitingHuman` | task.awaiting_human".to_string() @ crates/services/src/workflow/actions/dispatch.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ProjectCreated` | project.created".to_owned() @ crates/api/src/routes/projects.rs | REST SSE runtime allowlist | yes: `project.created` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `ProjectUpdated` | project.updated".to_owned() @ crates/api/src/routes/projects.rs | REST SSE runtime allowlist | yes: `project.updated` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `ProjectDeleted` | project.deleted".to_owned() @ crates/api/src/routes/projects.rs | REST SSE runtime allowlist | yes: `project.deleted` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `ProjectPaused` | project.paused".to_owned() @ crates/api/src/routes/projects.rs | REST SSE runtime allowlist | yes: `project.paused` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `ProjectResumed` | project.resumed".to_owned() @ crates/api/src/routes/projects.rs | REST SSE runtime allowlist | yes: `project.resumed` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `DaemonRegistered` | daemon.registered".to_owned() @ crates/services/src/daemon_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `DaemonConnected` | daemon.connected".to_owned() @ crates/services/src/daemon_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `DaemonReportReceived` | daemon.report_received".to_owned() @ crates/services/src/daemon_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `DaemonOffline` | daemon.offline".to_owned() @ crates/services/src/daemon_monitor.rs; daemon.offline".to_owned() @ crates/services/src/daemon_service.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `TransitionEffectFailed` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TransitionGuardRejected` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TransitionCascadeDepthExceeded` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `TaskRoleNotified` | task.role_notified".to_string() @ crates/services/src/workflow/actions/dispatch.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `NotificationCreated` | notification.created".to_owned() @ crates/services/src/notification_service.rs | REST SSE runtime allowlist | yes: `notification.created` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `OperationsStatusChanged` | OPERATIONS_STATUS_CHANGED_EVENT.to_string() @ crates/services/src/operator_status_emitter.rs | OperatorStatusEmitter, REST SSE runtime allowlist | yes: `operations.status_changed` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `ProjectHookRunChanged` | PROJECT_HOOK_RUN_CHANGED_EVENT.to_owned() @ crates/services/src/project_hooks/engine.rs | REST SSE runtime allowlist | yes: `project_hook.run_changed` with fixed safe payload | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; bounded notification projection. |
| `DomainEventCommitted` | domain_event.committed".to_owned() @ crates/services/src/domain_event_service.rs | ProjectHookService, SSE exact DomainEvent lookup, OrchestratorRuntime uses durable ledger directly | lookup exact DomainEvent id; wrapper itself suppressed | `exact row by event id; no inferred semantic alias` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP; exact receipt lookup only. |
| `AgentChatMessageAdmitted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `AgentChatTurnProgress` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `AgentChatResponseCompleted` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `AgentChatUpdated` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. REMOVE_PUBLIC; retain internal only while current consumers need it; PR13 owns physical cleanup. |
| `ExternalSyncCompleted` | external_sync.completed".to_owned() @ crates/services/src/external_sync.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `ExternalSyncFailed` | external_sync.failed".to_owned() @ crates/services/src/external_sync.rs | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |
| `Empty` | none found in non-test producer scan | no production consumer found; internal hint only | no; explicit SSE projection suppresses this hint | `—` | EventContext is never authority; DomainEvent row is the ledger when explicitly linked. TARGET_V2_KEEP internal signal; no public projection. |

`events.resync_required` is synthesized only after subscriber lag and remains
public with a bounded reason and skipped count. Legacy `task.status_changed`,
workflow effects/guards, assignment snapshots, recovery, legacy Review-row, and
Agent Chat variants are not in the SSE allowlist.
