---
created_at: 2026-08-14T22:34:11Z
updated_at: 2026-08-15T10:50:28Z
---

## Why

Forge already has one global Main Agent, one Project Agent per Project, durable chats, provider-backed identities, and protected credentials. The current web information architecture does not communicate that model cleanly:

- the global chat appears inside the generic Workspace section and repeats a global/project switcher inside the chat page, so it reads like one more workspace utility instead of the account-level Main Agent;
- the Project Agent chat looks like a second generic chat even though the Project Agent's job is to shape Project records and manage Tasks;
- agent profiles, provider identities, Main binding, Project binding, and runtime defaults are spread across several screens;
- connecting an agent requires pasting a secret even when the provider offers an interactive account authorization flow.

The result is unnecessary navigation and setup friction. This change makes the hierarchy visible, turns the Project Agent surface into a Project-management workbench, consolidates all agent configuration, and adds guided OpenAI, xAI/Grok, and Gemini connection flows without weakening Forge's authority or credential boundaries.

## Source Baseline

- `docs/spec/changes/add-project-agent-federation-2026-08-12/` is the implemented behavioral baseline for the singular Main Agent, Project Agents, chats, bindings, protected credentials, and Task-only repository workspaces.
- `../tui` is a design donor for browser PKCE, device authorization, renewable credential bundles, refresh, redaction, and direct provider adapters. Forge does not depend on TUI and does not import TUI or Codex credential files.
- OpenAI's authentication documentation describes Sign in with ChatGPT, browser login, device login, cached credentials, and token refresh for Codex clients: <https://developers.openai.com/codex/auth>.
- xAI documents API-key inference, an OAuth-capable CLI, and OAuth 2.0/OIDC infrastructure: <https://docs.x.ai/developers/rest-api-reference/inference>, <https://docs.x.ai/build/cli/reference>, and <https://docs.x.ai/build/enterprise>.
- Google documents OAuth for the Gemini API and installed applications: <https://ai.google.dev/gemini-api/docs/oauth> and <https://developers.google.com/identity/protocols/oauth2/native-app>.
- Gemini CLI explicitly disallows third-party use of the services that power Gemini CLI through Gemini CLI OAuth: <https://github.com/google-gemini/gemini-cli/blob/main/docs/resources/tos-privacy.md>. Forge therefore uses Gemini API OAuth or an AI Studio API key and never imports or reuses Gemini CLI/Code Assist credentials.

## What Changes

- **BREAKING — make Main Chat account-level navigation.** Render `Main Chat` immediately below the Project switcher and immediately above the `Project` section label. Remove its duplicate entry from `Workspace` and remove the nested global/project chat switcher from chat screens. The persistent launcher still opens the same global Main Agent timeline.
- Rename the Project navigation entry from `Project Chat` to `Agent Workspace` while retaining the existing `/projects/:projectId/chat` route. The surface combines the Project Agent timeline with typed editing affordances for Project records, Decisions, artifacts, and Tasks. It is not a repository editor: code and filesystem mutation continue only through assigned Task Workspaces.
- **BREAKING — consolidate agent configuration.** Replace the separate legacy agent list, federated identity page, Main binding card, and Project-local `Project Agent` settings tab with one account-level `Agent Settings` surface. It inventories every available agent/profile, connection, health state, credential method, and Main/Project binding. Project-context links open the same surface with a Project filter; they do not create a second settings model.
- **BREAKING — separate provider entries from agents.** Agent Settings presents two tabs over one model: `Providers` — an inventory of credentialed provider entries (multiple entries per provider type are allowed, e.g. two OpenAI accounts) plus discovered CLI-managed runtimes with authentication health and usage — and `Agents`, the runnable roster. Connecting a provider never creates an agent; agent creation is a guided wizard that references a provider entry (or CLI runtime), a runtime kind (`direct` or a harness such as Codex CLI/Claude Code), and configuration. The former single-shot connect-and-create contract is split into provider-entry and agent-creation operations, with existing identities migrated forward into entry + agent pairs.
- Add a provider capability catalog so the UI can display supported login methods, runtime compatibility, and support status before asking for credentials.
- Add short-lived, account-owned provider authorization operations. The server owns PKCE verifiers, device codes, polling, token exchange, refresh, and cancellation; the browser receives only public URLs/codes and redacted operation state.
- Add `Continue with ChatGPT` using browser PKCE with a device-code fallback. Direct ChatGPT subscription access is an explicit experimental compatibility surface, separate from the stable OpenAI Platform API-key path.
- Add `Continue with xAI` using the xAI device authorization flow. Subscription-backed direct Grok access is experimental until xAI documents the same backend and client contract for third-party applications; the xAI API-key path remains available.
- Add `Continue with Google` using supported Gemini API OAuth and a registered Forge Google OAuth client. Development builds without registered client configuration present a precise setup state and retain the Gemini AI Studio API-key path. Forge never uses Gemini CLI/Code Assist OAuth as a provider credential.
- Store versioned OAuth bundles in Forge's existing encrypted protected-credential store. Refresh-token rotation is atomic, concurrent refresh is single-flight, revocation/disconnect is explicit, and secrets never appear in API payloads, logs, events, or rendered errors.
- Publish or update a provider entry only after authorization and provider discovery succeed; immutable agent profiles are published by explicit agent creation referencing an entry. Failed, expired, cancelled, or revoked operations do not create a half-connected entry or profile and do not change an existing active binding.
- Add a new numbered migration beginning at `V078` only if the implementation needs credential-kind or authorization-operation persistence. Existing API-key credentials are preserved and classified without editing historical migrations.
- Update the REST types/routes, generated TypeScript, API documentation, architecture documentation, getting-started guidance, and `CHANGELOG.md` together. The navigation/settings removals receive an `Unreleased` `### Breaking` entry.

## Important Reconciliations

- This change supersedes only the `Chat Switcher and Global Launcher` navigation requirement and the distributed settings/setup presentation established by `add-project-agent-federation`. It does not loosen the singular binding, scope, memory, handoff, or Task authority requirements from that change.
- `Agent Workspace` means editing Forge Project data through typed services. A Project Agent chat still receives no repository path, shell, raw filesystem tool, or Workspace lease.
- One Agent Settings surface may present several underlying agent kinds and credential methods. It does not collapse stable identity, immutable profile, binding, runtime-default, or Task-assignment domain concepts.
- The existing protected store is authoritative. Forge does not read `~/.codex/auth.json`, Gemini CLI storage, TUI auth files, browser cookies, or another application's keychain entries.
- Provider login does not sign a person into Forge. Forge user authentication remains separate from provider authorization.
- Experimental provider adapters are visibly labeled and never replace documented API-key paths. Provider-specific breakage fails locally and does not corrupt bindings or shared credentials.
- No compatibility alias, duplicate settings page, or `_v2` route is retained. Contextual links target the canonical surface.

## Impact

- Affected specs: `agent-chats`, `project-agent-workbench`, `agent-settings`, and `provider-authentication`.
- Affected web: application shell, chat page, Project navigation, Project settings, Agent inventory/settings, provider connection dialog, callback/polling states, mobile layout, and global launcher.
- Affected backend: provider capability registry, authorization operation service, protected credential schema and rotation, embedded agent connection/profile publication, binding reads/writes, and provider runtime adapters.
- Affected public surfaces: provider capability and authorization-operation REST contracts; generated TypeScript; removal or relocation of duplicate web routes; provider and credential-kind enums.
- Affected persistence: a forward-only migration from `V078` if durable operation or credential metadata is required. User API keys, profiles, bindings, and chats are preserved.
- Affected documentation: root `DESIGN.md`, `docs/architecture.md`, `docs/api.md`, `docs/getting-started.md`, README links if necessary, and `CHANGELOG.md`.

## Non-Goals

- Forge account SSO or replacing Forge's email/password user authentication.
- A repository file editor, terminal, shell, or unrestricted filesystem tools in Main or Project Agent chat.
- Multiple Main Agents, multiple persistent Project Agents per Project, Rooms, or arbitrary chat threads.
- Implicit cross-project authority, shared private memory, or changing Task Worker/reviewer Workspace rules.
- Importing, syncing, or depending on credentials owned by Codex, TUI, Gemini CLI, xAI CLI, a browser profile, or another application.
- Treating Gemini CLI/Code Assist subscription credentials as Gemini API credentials.
- Removing API-key connection methods.
- Shipping a provider's experimental direct subscription adapter as a guaranteed public provider contract.

## Approval Gate

This is a Stage 1 proposal. Approval authorizes the implementation described by this proposal, `design.md`, `tasks.md`, and the delta specifications in this change directory. It does not authorize broader agent authority, repository mutation from chat, reuse of another application's OAuth credentials, or release before the listed validation gates pass.
