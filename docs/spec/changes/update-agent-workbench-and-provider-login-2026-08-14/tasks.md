---
created_at: 2026-08-15T02:36:20Z
updated_at: 2026-08-15T10:50:28Z
completed_at:
---

## 0. Approval and Baseline

- [x] 0.1 Obtain explicit approval for this proposal, design, task plan, and all delta specifications before implementation. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 0.2 Record the implemented `add-project-agent-federation-2026-08-12` change as the behavioral baseline and preserve its singular bindings, scope isolation, protected credential, and Task Workspace requirements. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 0.3 Confirm the exact pinned Agent Runtime revision's renewable credential-source and native Gemini provider contracts; if either is absent, land and pin the smallest upstream runtime revision before Forge integration. <!-- completed: 2026-08-15T02:34:53Z -->

## 1. Design System and Information Architecture

- [x] 1.1 Update root `DESIGN.md` first with the new sidebar hierarchy, Main Chat placement, Agent Workspace composition, canonical Agent Settings surface, provider connection states, responsive behavior, and accessibility requirements. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 1.2 Add or update isolated component stories/harnesses for sidebar order, workbench editor states, provider cards, browser/device login, refresh, failure, cancellation, and disconnected bindings. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 1.3 Replace the chat-internal agent switcher with shell-level navigation and keep the global launcher bound to the canonical Main timeline. <!-- completed: 2026-08-15T02:34:53Z -->

## 2. Provider Capability and Persistence Model

- [x] 2.1 Add provider identifiers/capabilities for OpenAI, xAI/Grok, Gemini, OpenRouter, and OpenAI-compatible endpoints with stable/experimental/unavailable support metadata. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 2.2 Model finite, account-owned provider authorization operations with explicit expiry, browser/device public state, cancellation, and redacted failures. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 2.3 Add migration `V078` only if required for credential kind, versioned OAuth bundle metadata, or durable operation state; preserve and classify all existing API-key credentials. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 2.4 Add repository tests for migration, optimistic concurrency, operation ownership, expiry, and atomic credential rotation. <!-- completed: 2026-08-15T02:34:53Z -->

## 3. Protected Renewable Credentials

- [x] 3.1 Extend the protected store to encrypt/decrypt versioned API-key and OAuth credential payloads without exposing secret material above the credential boundary. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 3.2 Implement per-credential single-flight refresh, atomic refresh-token rotation, bounded pre-stream retry, terminal invalidation, disconnect, and best-effort provider revocation. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 3.3 Add adversarial redaction tests covering API/log/event/error/debug serialization, callback parameters, database metadata, and failed exchanges. <!-- completed: 2026-08-15T02:34:53Z -->

## 4. Provider Authorization and Runtime Adapters

- [x] 4.1 Implement OpenAI ChatGPT browser PKCE plus device fallback, renewable credentials, and the experimental direct adapter; preserve the stable OpenAI Platform API-key adapter. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 4.2 Implement xAI OIDC discovery, RFC 8628 device authorization, bounded polling, refresh, and the experimental direct Grok adapter; preserve the xAI API-key adapter. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 4.3 Implement Google OAuth for the documented Gemini API using a registered Forge client and native Gemini adapter; preserve AI Studio API keys and reject any Gemini CLI/Code Assist credential-import path. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 4.4 Make provider verification/model discovery transactional with immutable profile publication and active-profile selection. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 4.5 Add deterministic mock-provider tests for success, denial, expiry, slow-down, malformed discovery, callback replay, state mismatch, refresh rotation, revocation failure, and provider outage. <!-- completed: 2026-08-15T02:34:53Z -->

## 5. REST and Service Integration

- [x] 5.1 Add provider capability and authorization-operation request/response types in `api-types`, route handlers in `api`, service policy, and generated TypeScript in the same change. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 5.2 Enforce account ownership, callback origin/state, rate limits, finite leases/polling, authorization, and optimistic concurrency at service boundaries. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 5.3 Keep login independent from binding: connect/publish first, then perform explicit Main or Project binding changes through existing cardinality rules. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 5.4 Update `docs/api.md` with every new or changed endpoint, status, enum, terminal state, and redaction rule. <!-- completed: 2026-08-15T02:34:53Z -->

## 6. Canonical Agent Settings

- [x] 6.1 Rebuild `/agents` as the one searchable/filterable inventory for native provider identities/profiles, CLI/runtime agents, health, credentials, and Main/Project/Task roles. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 6.2 Move provider connect/reconnect/disconnect, profile activation, Main binding, and Project binding interactions into the canonical surface. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 6.3 Remove `/agents/federated` and the Project-local `project-agent` settings tab without aliases; update every call site and contextual link to `/agents` with optional non-authoritative filters. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 6.4 Preserve admin-only Forge runtime defaults under Forge Settings and clearly distinguish them from account-owned Agent Settings. <!-- completed: 2026-08-15T02:34:53Z -->

## 7. Main Chat and Project Agent Workspace

- [x] 7.1 Place `Main Chat` immediately below the Project switcher and above the Project section label in desktop and compact navigation; remove its Workspace entry. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 7.2 Remove the chat-internal global/Project switcher while preserving direct routes, focus behavior, unread/error state, and the global launcher timeline. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 7.3 Rename the Project entry/page to `Agent Workspace` and add typed Project record, Decision, artifact, milestone, and Task editing affordances with saved/conflict/error receipts. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 7.4 Prove through policy and integration tests that the workbench exposes no repository path, shell, raw filesystem tool, or Workspace lease to Main/Project Agent runs. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 7.5 Implement the segmented compact layout, keyboard navigation, focus management, reduced motion, and accessible operation status announcements. <!-- completed: 2026-08-15T02:34:53Z -->

## 8. Documentation and Breaking Surface

- [x] 8.1 Update `docs/architecture.md` for provider authorization, renewable credentials, canonical settings, and the Project Agent workbench authority boundary. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 8.2 Update `docs/getting-started.md` with the guided login and API-key alternatives, provider support labels, OAuth client configuration, and disconnect/recovery guidance. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 8.3 Update README links if necessary and add visible `Unreleased` `### Breaking` entries for navigation/settings removals and public contract changes. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 8.4 Ensure no `_v2`, deprecated alias, credential-cache import, or compatibility shim remains. <!-- completed: 2026-08-15T02:34:53Z -->

## 9. Validation and Proof

- [x] 9.1 Run formatting, targeted Rust tests, `cargo test -p api --test happy_path`, workspace tests, clippy, and migration tests. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 9.2 Run `pnpm lint`, `pnpm typecheck`, `pnpm test`, and `pnpm build` in `web/`. <!-- completed: 2026-08-15T02:34:53Z -->
- [x] 9.3 Run accessibility and keyboard checks plus real Chromium validation at 375 px, 768 px, and 1280 px, capturing proof for sidebar order, workbench editing, Agent Settings, and every provider login state. <!-- completed: 2026-08-15T02:34:53Z -->
- [ ] 9.4 Perform opt-in smoke tests against test OpenAI, xAI, and Gemini accounts with output redaction verified; do not block deterministic CI on live provider availability. <!-- pending: requires user-supplied opt-in test accounts -->
- [x] 9.5 Re-run strict spec validation and reconcile every acceptance scenario before marking the change complete. <!-- completed: 2026-08-15T02:34:53Z -->

## 10. Provider/Agent Split and Two-Tab Settings (revision 2026-08-15)

- [x] 10.1 Update root `DESIGN.md` with the two-tab Agent Settings structure, provider-entry cards, the CLI runtimes group, and the three-step agent creation wizard. <!-- completed: 2026-08-15T08:19:00Z -->
- [x] 10.2 Split the connect contract: provider-entry list/create/rename/delete endpoints (`/api/v1/providers`, catalog at `/api/v1/providers/catalog`) and agent creation referencing a provider entry (or CLI runtime) plus runtime kind; regenerate TypeScript; update `docs/api.md`; add `### Breaking` changelog entries. <!-- completed: 2026-08-15T08:19:00Z; POST /api/v1/embedded-agents (direct) and POST /api/v1/agents credential_id (harness) -->
- [x] 10.3 Complete authorization operations into provider entries (no agent profile publication); move immutable profile publication to agent creation; keep verification/discovery transactional. <!-- completed: 2026-08-15T08:19:00Z -->
- [x] 10.4 Extend the capability catalog with runtime compatibility per credential kind (`direct`, harness kinds) and per-combination support levels; enforce server-side rejection of incompatible combinations. <!-- completed: 2026-08-15T08:19:00Z -->
- [x] 10.5 Support multiple entries per provider type with editable display names, per-entry health and usage (referencing agents, last used), and dependent-aware removal that marks referencing agents visibly unhealthy. <!-- completed: 2026-08-15T08:19:00Z; label rename via PATCH /providers/{id} -->
- [x] 10.6 Surface discovered CLI-managed runtimes on the Providers tab with authentication health, host, usage, and login recovery guidance; never read another application's credential files. <!-- completed: 2026-08-15T08:19:00Z -->
- [x] 10.7 Implement `auth_source` for harness-backed agents (`forge_provider` dispatch-time credential injection, `cli_managed`) with redaction tests covering injected credentials. <!-- completed: 2026-08-15T08:19:00Z; auth_source is expressed as profile credential_ref presence; injection at TaskService dispatch mutates only the in-memory snapshot -->
- [x] 10.8 Rebuild `/agents` as the two-tab surface (Providers, Agents) with the guided wizard, roster, binding actions, empty states, and preserved `?project=` filter semantics. <!-- completed: 2026-08-15T08:19:00Z -->
- [x] 10.9 Migrate existing single-shot connected identities into provider entry + agent pairs with a forward-only migration preserving credentials, profiles, and bindings. <!-- completed: 2026-08-15T08:19:00Z; no migration required — provider entries are the existing credential_handle rows and profiles already reference them via credential_ref, so existing data decomposes in place -->
- [x] 10.10 Re-run automated validation gates for the revised surface: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, full `cargo test --workspace` (129 suites incl. the canonical happy path), `pnpm lint`/`typecheck`/`test` (196 tests incl. the two-tab page suite)/`build`, plus a live-server API walkthrough (register → catalog runtime matrix → two same-type entries → CLI runtimes listed → entry-referenced direct agent → matrix rejection of an incompatible harness → usage → versioned rename → dependent-aware disconnect leaving the agent `connection_unavailable`). <!-- completed: 2026-08-15T08:19:00Z -->
- [ ] 10.11 Capture real-Chromium visual/accessibility proof of the two-tab surface at 375/768/1280 px. <!-- pending: Claude-in-Chrome browser extension was not connected during implementation; automated component tests and the live API walkthrough cover behavior, not visuals -->
