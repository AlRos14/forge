---
created_at: 2026-08-12T19:22:52Z
updated_at: 2026-08-13T07:43:39Z
completed_at: 2026-08-13T07:43:39Z
---

## 0. Preserved implementation and acceptance baseline

These completed items are retained foundations, not approval of the superseded Room product surface.

- [x] 0.1 Pin and publish Agent Runtime revision `a7075b1d2dd1cee05db63bc480ff46b0f97ec239`; validate remote-source resolution and the neutral Open Forge consumer gate.
- [x] 0.2 Implement and test stable identities, immutable profiles, protected credential references, Forge-hosted native sessions, scoped LCM/context manifests, scoped memory, commitments, typed actions, durable events, Attention, Agent detail, and Mission Control foundations.
- [x] 0.3 Prove the native Task Worker/reviewer path remains constrained by existing Task assignment, Workspace, validation, review, and delivery authority.
- [x] 0.4 Complete a live Smith Todo Task acceptance from Project/repository creation through execution, validation, review, merge, history, and proof attachment; preserve the report at `test/live-agent-20260813-0057/ACCEPTANCE.md`.
- [x] 0.5 Complete `ap-browser` baseline acceptance at 1280/768/375 and record the silent leased-turn failure, primary-Worker invariant bug, mobile overflow/length issues, empty run-chat projection, and attached proof media.

## 1. Revised specification and approval gate

- [x] 1.1 Replace the Room/participant/round product contract with one Main Agent, exactly one Project Agent per operational Project, singular Agent Chats, and explicit handoff across proposal, design, and delta specs.
- [x] 1.2 Define hard Main/Project/Task tool boundaries, setup-required migration behavior, finite visible turn state, new `V071+` migration rule, chat switcher, global launcher, and Product Genesis reconciliation.
- [x] 1.3 Strictly validate the revised change and reconcile `add-product-genesis-chat` with the Main-chat/handoff model.
- [x] 1.4 Obtain explicit user approval of the revised proposal before writing implementation code.

## 2. Schema and data-preserving migration

- [x] 2.1 Add new numbered migration(s) beginning at `V071`; do not edit `V059`–`V070` or any earlier migration.
- [x] 2.2 Add versioned `account_main_agent_binding` and `project_agent_binding` persistence with unconditional unique-active invariants and replacement history.
- [x] 2.3 Add singular `agent_chat`, immutable `agent_chat_message`, bounded `agent_chat_turn_job`, and immutable `agent_handoff` persistence plus repository traits/models/row mapping.
- [x] 2.4 Create one global chat per account and one Project chat per Project; add explicit `agent_setup_required` handling for ambiguous imported Projects.
- [x] 2.5 Migrate all legacy Conversation and pre-release Room messages, metadata, instruction provenance, sessions, LCM, memory, turn jobs, and protected-content audit links without changing message IDs/bodies.
- [x] 2.6 Deterministically merge multiple source threads by timestamp/source ID/source sequence while preserving original thread boundaries as provenance.
- [x] 2.7 Infer bindings only from one safe eligible explicit responder/Steward; never promote a primary Worker. Convert ambiguous/expired leases into visible finite retry/terminal state.
- [x] 2.8 Add migration integrity tests for empty/legacy/multi-Room/ambiguous/protected/leased fixtures and prove Project, Task, message, memory, and evidence preservation.

## 3. Bindings, chat, handoff, and turn services

- [x] 3.1 Replace Project membership/primary role service paths with singular Main/Project binding services using optimistic concurrency.
- [x] 3.2 Implement atomic new-Project creation with exactly one Project Agent binding, Project chat, and matching domain events.
- [x] 3.3 Replace `RoomService` and Room scheduler with `AgentChatService`: immutable admission, one current-binding responder, one live turn per trigger, no recursive reply admission.
- [x] 3.4 Implement finite turn leases/attempts/backoff, idempotent success/failure commit, deterministic expired-lease recovery, terminal failure, cancellation, and explicit user-visible status.
- [x] 3.5 Reproduce SQLite failure-persistence contention and prove no turn remains silently leased or retries beyond its stored budget across restart.
- [x] 3.6 Implement immutable typed Main-to-Project handoff with guarded bounded content, source/target provenance, delivery receipt, one target turn, and replay idempotency.
- [x] 3.7 Delete Room participants, addressing, default-responder policy, bounded rounds, and Room-only service/scheduler/domain concepts after migration verification.

## 4. Scope-derived authority and Task delegation

- [x] 4.1 Define server-issued Main Agent read/action catalogs for discovery, configured web search, Project lifecycle/organization, bounded portfolio summaries, and handoff only.
- [x] 4.2 Add hard policy tests proving Main Agent cannot create/edit/assign/transition/review/merge/deliver Tasks or access repositories, even with forged IDs/prompts/references.
- [x] 4.3 Define Project Agent catalogs for Project setup, decisions, commitments, memory, and Task management only within its bound Project.
- [x] 4.4 Route allowed Project Agent Task actions through existing `TaskService`/workflow with policy, contract, budget, version, and deduplication validation.
- [x] 4.5 Prove Project Agent cross-project denial and deny-all core-chat filesystem; retain repository mutation only for admitted Task Workers/reviewers.
- [x] 4.6 Keep Task Worker/reviewer identities out of persistent chat bindings/switcher unless separately and explicitly selected as Main/Project Agent.

## 5. Runtime, LCM, memory, commitments, and events

- [x] 5.1 Remap canonical conversation scopes from Room to owning Agent Chat while preserving Task-native scope and historical attribution.
- [x] 5.2 Migrate and test per-identity Chat timelines so binding/profile/session replacement preserves correct continuity without merging identities or scopes.
- [x] 5.3 Update deterministic domain-source selection, `ContextManifest`, Agent Runtime `RunManifest` linkage, provenance inspector, and semantic-memory deduplication for Main/Project Chat scopes.
- [x] 5.4 Replace Room memory ACL/history rules with account/Agent Chat/Project/Task rules and explicit immutable handoff publication.
- [x] 5.5 Update commitments/inbox/current-focus projection for Main discovery/handoff and Project Task outcomes; require explicit transfer across binding replacement.
- [x] 5.6 Update transactional events, consumers, Attention categories, and bounded wakes for binding setup, chat retry/failure, and handoff status; prove replay idempotency.
- [x] 5.7 Re-run Agent Runtime LCM conformance, protected-state redaction, cross-scope isolation, deterministic fingerprint, restart, and session-rotation gates.

## 6. REST, MCP, CLI, and public types

- [x] 6.1 Add singular Main Agent and Project Agent binding REST resources with version-conflict behavior.
- [x] 6.2 Add authorized Agent Chat switcher/detail/message/turn resources and explicit Project handoff resource.
- [x] 6.3 Delete Room routes/types/event names and add synchronized api-types plus generated TypeScript bindings; do not retain aliases.
- [x] 6.4 Replace Room MCP tools with least-authority Agent Chat/handoff tools and bind agent-internal tools to server-issued canonical scope rather than caller authority IDs.
- [x] 6.5 Replace Room `forge-ctl` commands with approved Main/Project binding, chat inspection/send, handoff, session, context, and commitment commands.
- [x] 6.6 Update public API behavior tests for cardinality, setup-required, replacement, pagination/cursors, authorization, idempotency, finite turn state, and handoff.

## 7. Web product surface

- [x] 7.1 Replace Room navigation with a compact left chat switcher containing Global/Main plus one entry per authorized Project Agent Chat.
- [x] 7.2 Add an always-available bottom-right launcher for the same global timeline; opening it SHALL neither fork a chat nor import current Project-private context.
- [x] 7.3 Build the Main chat experience for discovery, web-search status, Project creation/organization, bounded summaries, handoff receipts, and “Continue with Project Agent.”
- [x] 7.4 Build the Project chat experience for setup, decisions, commitments, Task creation/management, delivery outcomes, context provenance, and setup-required state.
- [x] 7.5 Render queued/leased/retrying/failed/cancelled/succeeded responder state beside the triggering message with policy-allowed recovery controls.
- [x] 7.6 Remove Room creation, participants, addressing, responder policy, and round controls from the UI.
- [x] 7.7 Refocus Mission Control and Agent detail on Main/Project bindings, relevant Task agents, Attention, and outcomes rather than all configured profiles.
- [x] 7.8 Add frontend unit/integration tests for switcher identity, shared global launcher timeline, scope isolation, handoff navigation, setup-required, finite failure state, live invalidation, and long-content containment.

## 8. Product Genesis and documentation

- [x] 8.1 Rebase `add-product-genesis-chat` so Genesis is a typed discovery protocol in the existing Main Chat, not a new Conversation/Room/Chat.
- [x] 8.2 Make Genesis Project creation use the atomic Project-Agent binding path and publish its approved brief through the normal handoff; never grant Task tools to Main scope.
- [x] 8.3 Update `docs/architecture.md` for singular bindings/chats, authority matrix, turn lifecycle, handoff, migrations, runtime/memory, Task outcome reconciliation, and event flow.
- [x] 8.4 Update every REST/MCP shape together with `docs/api.md`; update `docs/cli.md`, `docs/getting-started.md`, and README links/summary where necessary.
- [x] 8.5 Add `CHANGELOG.md` `Unreleased` `### Breaking` entries for removal of Room/membership surfaces and the singular Agent Chat/binding replacement.

## 9. Automated validation

- [x] 9.1 Run `cargo fmt --all`.
- [x] 9.2 Run `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] 9.3 Run `cargo test`, including `cargo test -p api --test happy_path`, migration fixtures, concurrency races, and failure-contention regression.
- [x] 9.4 Run `cd web && pnpm lint && pnpm typecheck && pnpm test && pnpm build`.
- [x] 9.5 Re-run exact remote Agent Runtime source resolution and Open Forge neutral/product consumer gates without relying on committed sibling paths.

## 10. End-to-end release acceptance

- [x] 10.1 From a clean `./test` root, connect/select a Main Agent and a different Project Agent directly in Forge with protected credentials.
- [x] 10.2 In the global chat, discover a Todo product, exercise configured web search or a deterministic stub, create the Project atomically, and publish a visible handoff.
- [x] 10.3 Switch to the Project Agent chat, finish setup, create/manage a Todo Task, assign an existing compatible Worker/reviewer, and prove the Main Agent cannot mutate the Task.
- [x] 10.4 Execute, validate, review, merge, reconcile evidence/commitments, and inspect Task history plus raw/chat run projections without bypassing Task Workspace authority.
- [x] 10.5 Restart Forge during queued/leased/retrying chat turns and prove finite visible recovery, no duplicate response, no silent lease, and no reset attempt budget.
- [x] 10.6 Capture browser proof with `ap-browser` at 1280/768/375 for direct connection, global launcher, switcher, handoff, Project Task management, failure/retry state, Agent detail, provenance, Mission Control, accessibility, dark mode, and no horizontal overflow.
- [x] 10.7 Attach representative proof to the live acceptance Task/comment and verify every uploaded URL; record exact IDs, revisions, commands, and outcomes in an updated acceptance report.
- [x] 10.8 Strictly validate the final delta specs, synchronize status/timestamps, request shipping approval, and do not archive until deployed/shipped.
