## Context

The implemented federation model is conceptually simple: one account-level Main Agent, one Project Agent for each Project, and Task-scoped Workers/reviewers. The current UI overlays a second hierarchy on that model by placing global chat in `Workspace`, rendering another global/project switcher inside the chat page, and distributing agent configuration across agent, federation, Project settings, and system settings screens.

Provider connection currently accepts static credentials for OpenAI, OpenAI-compatible endpoints, and OpenRouter. Forge already encrypts protected credential material, but it does not model renewable OAuth bundles or transient authorization operations.

## Goals

- Make account, Project, and workspace scope visually obvious before the user opens a screen.
- Make the Project Agent feel like a Project editor and orchestrator without granting repository access.
- Give users one place to understand and configure every available agent.
- Make provider connection a guided login when a supported flow exists, with API keys as an explicit alternative.
- Preserve credential confidentiality, profile immutability, binding cardinality, and Task Workspace authority.

## Target Information Architecture

Desktop sidebar order:

```text
Project switcher
Main Chat

PROJECT
  Overview
  Board
  Tasks
  Agent Workspace
  Project Settings

WORKSPACE
  Agent Settings
  Mission Control
  Daemons
  Operations
  Forge Settings
```

`Main Chat` is always visible after the Project switcher and before the Project label. It is account-scoped even while a Project is selected. `Agent Workspace` always addresses the selected Project's one Project Agent. `Agent Settings` is the single configuration surface for identities, profiles, credentials, connections, and bindings. `Forge Settings` retains server/runtime/path administration and is not another agent inventory.

On compact screens, the same order is preserved in the navigation drawer. The global launcher opens the canonical `/chat` timeline and does not create a modal-only conversation.

## Chat and Workbench Surfaces

### Main Chat

`/chat` renders the Main Agent timeline and its account/portfolio affordances. The page does not render a second agent roster or Project chat switcher. Project selection remains in the application shell, and explicit typed Project references/handoffs preserve scope.

### Project Agent Workspace

`/projects/:projectId/chat` remains the canonical route so existing deep links and chat identity do not change, but the navigation label and page title become `Agent Workspace`.

The desktop surface has two coordinated regions:

- a durable Project Agent conversation with turn status, references, and typed action receipts;
- a Project editing rail for Project summary/status, Decisions, artifacts, milestones, and Tasks that the current user may change.

The agent and user edit Project state only through typed Forge services. Optimistic edits show pending/saved/conflict/error states; version conflicts never silently overwrite newer data. On mobile, conversation and Project editing become a segmented, keyboard-accessible view rather than compressed side-by-side panes.

No Main or Project Agent chat session receives a repository root, worktree, shell, raw file tool, or Workspace lease. When implementation is needed, the Project Agent creates or manages a traceable Task and Task Workers operate inside the existing Task Workspace boundary.

## Canonical Agent Settings

The canonical route is `/agents`. It replaces `/agents/federated` as a separate destination and replaces the `project-agent` Project settings tab. No compatibility alias or duplicate page is kept.

The surface is organized as two tabs over one settings model: `Providers` and `Agents`. Contextual setup links use the same route with a non-authoritative query filter such as `/agents?project=<id>`, which opens the Agents tab with a Project context filter. The server still enforces account ownership, Project access, admin-only runtime settings, cardinality, and optimistic concurrency. A query string never grants authority.

### Providers tab

The Providers tab is an inventory of configured provider entries, not a fixed per-provider checklist. Each entry is one credentialed connection: provider type, credential method, an editable display name, discovered account identity, connection/authorization health, and usage (which agents reference it and when it was last used). A user may configure multiple entries of the same provider type — two OpenAI entries with different accounts, or a Google OAuth Gemini entry alongside an AI Studio key entry. A second entry of the same type receives an auto-suffixed, editable display name.

`Add provider` opens the capability catalog picker (provider types with stable/experimental/unavailable labels), then the method chooser (guided login primary, API key alternative), then the existing authorization operation states. A completed authorization or API-key submission produces a provider entry — it never creates an agent. The terminal success state offers `Create an agent with this provider`, which opens the Agents tab wizard with the entry preselected.

Discovered CLI-managed runtimes (Codex CLI, Claude Code, Cursor, Gemini CLI, OpenCode) appear in a `CLI runtimes` group on the same tab so every authentication source is visible in one place. These entries come from runtime/daemon discovery rather than the catalog picker; they show CLI authentication health (including the existing `CLI not authenticated` state), the host runtime, and the same usage display. An unauthenticated CLI shows recovery guidance naming the login command and host. Forge reads only health signals — it never imports another application's credential files.

Removing a provider entry that agents still reference warns with the dependent list first; after removal those agents become visibly unhealthy and are never silently rebound.

### Agents tab

The Agents tab is the roster of runnable agents. Each agent references exactly one authentication source — a provider entry or a CLI-managed runtime — plus a runtime kind and configuration. The searchable/filterable roster shows name, runtime kind, provider entry, model, Main/Project/Task roles, and health. Main and Project binding actions, profile revision activation, and per-agent defaults live here.

`New agent` is a three-step wizard:

1. **Authentication source** — choose a provider entry or CLI-managed runtime. With none configured, an inline `Add provider` action runs the Providers flow without losing wizard state and returns with the new entry selected.
2. **Runtime** — choose how the source is used: `Direct` (the embedded native adapter) or a harness (Codex CLI, Claude Code, and other executors). Options come from the server capability catalog per provider type and credential kind; incompatible options are visible but disabled with a user-safe reason. Direct use of subscription-backed OAuth bundles and OAuth handoff into a harness carry the experimental label; API-key paths are stable.
3. **Configure** — name, model (discovered from the source where supported), reasoning effort, permission policy, and system prompt with sane defaults on one screen.

Creation publishes the immutable profile and returns to the roster; if no Main Agent binding exists, the roster offers an explicit `Bind as Main Agent` action. Connecting a provider, creating an agent, and changing a binding remain three separate authorized operations.

### Provider and agent contract split

Connecting a provider and creating an agent are separate public operations. The former single-shot connect contract (credential + model + agent name in one request) is split: provider entries are created by authorization operations or API-key submission, and agent creation references an existing provider entry (or CLI runtime) plus runtime kind and configuration. Existing single-shot identities migrate forward into a provider entry plus an agent referencing it, preserving credentials, profiles, and bindings. Harness-backed agents record an `auth_source` of `forge_provider` (Forge injects the credential at dispatch; the flow is Forge→harness only) or `cli_managed` (the harness's own login).

## Provider Capability Model

The backend owns a capability registry rather than making the web app infer behavior from provider names. Each provider advertises:

- stable provider identifier and display name;
- supported credential methods (`api_key`, `browser_oauth`, `device_oauth`);
- support level (`stable`, `experimental`, `unavailable`);
- runtime compatibility per credential method (`direct` and harness kinds such as `codex`, `claude_code`) with a per-combination support level and user-safe unavailability reason;
- whether model discovery is available;
- whether a registered OAuth client or server configuration is missing;
- user-safe setup guidance.

The initial matrix is:

| Provider | Guided login | Alternative | Runtime boundary |
|---|---|---|---|
| OpenAI | ChatGPT browser PKCE; device fallback | OpenAI Platform API key | Direct ChatGPT subscription backend is experimental; Platform API key is stable |
| xAI / Grok | xAI device authorization | xAI API key | Subscription-backed direct adapter is experimental until documented for third-party clients |
| Gemini | Google OAuth for the Gemini API through a registered Forge client | Gemini AI Studio API key | Official Gemini API only; Gemini CLI/Code Assist OAuth is prohibited |
| OpenRouter | None initially | OpenRouter API key | Existing OpenAI-compatible path |
| OpenAI-compatible | None | Endpoint plus API key | Existing custom endpoint path |

The initial runtime-compatibility matrix enables `direct` for every connected provider entry (experimental where the credential is a subscription-backed OAuth bundle), enables the Codex harness for OpenAI entries (API key stable, OAuth handoff experimental), and leaves `cli_managed` login available for every harness independent of provider entries.

## Authorization Operation Lifecycle

The API exposes a provider-neutral operation contract:

1. The client requests an operation for a provider and login method.
2. The server validates account ownership, capability availability, callback origin, and required OAuth client configuration.
3. The server returns a redacted operation view: operation ID, state, expiry, public authorization URL, optional user code, and polling guidance.
4. The user completes browser or device authorization. The server owns the PKCE verifier/device secret, state validation, exchange, and provider polling.
5. Forge encrypts the renewable credential bundle, verifies provider access, discovers account/model metadata where supported, publishes or updates the provider entry, and only then reports success. No agent profile is published by authorization; profiles are published later by explicit agent creation referencing the entry.
6. Expired, denied, failed, or cancelled operations reach a finite terminal state and release transient secret material.

Representative public endpoints are:

```text
GET    /api/v1/providers/catalog                  (capability catalog; replaces /agent-providers)
GET    /api/v1/providers                          (configured provider entries + discovered CLI runtimes)
POST   /api/v1/providers                          (API-key entry; OAuth entries come from operations)
PATCH  /api/v1/providers/:providerId              (display name)
DELETE /api/v1/providers/:providerId
POST   /api/v1/provider-authorizations
GET    /api/v1/provider-authorizations/:operationId
POST   /api/v1/provider-authorizations/:operationId/cancel
GET    /api/v1/provider-authorizations/:provider/callback
POST   /api/v1/agents                             (references provider entry or CLI runtime + runtime kind)
```

The exact request/response shapes are authored in `api-types`, generated into TypeScript, and documented together. Callback endpoints validate state, bind the operation to its initiating account and redirect origin, and render only a close/return result. They never include tokens in a browser response.

OpenAI uses browser PKCE first and offers device authorization where the account/provider supports it. xAI uses RFC 8628 device authorization. Gemini uses the supported Google OAuth installed/web application flow for a Forge-owned registered client. A development build without Gemini OAuth client configuration reports that login as unavailable and offers the API-key method.

## Credential Storage and Refresh

The existing encrypted protected-credential store remains the sole durable secret owner. A protected credential gains an explicit kind and a versioned encrypted payload:

```text
api_key:      secret
oauth_bundle: access token, refresh token, expiry, scopes, provider account id, schema version
```

If durable metadata requires schema changes, migration `V078` adds them and classifies existing protected credentials as `api_key` without re-encrypting or dropping user data. Historical migrations remain untouched.

Runtime adapters consume a credential source rather than receiving a one-time access token. Refresh is single-flight per credential; refresh-token rotation and access-token update commit atomically using the existing version/concurrency discipline. A provider authentication failure may trigger one refresh/retry before semantic streaming begins. Forge never repeats a request after assistant output has started.

Disconnect marks the connection unusable before attempting provider revocation, clears protected token material transactionally, preserves non-secret audit provenance, and makes affected bindings visibly unavailable rather than silently rebinding them.

## Provider Adapter Decisions

### OpenAI

Adapt the browser PKCE, device flow, renewable bundle, and direct ChatGPT Responses patterns from `../tui`, but keep an independent Forge implementation and Forge-owned encrypted storage. This adapter is labeled experimental because OpenAI documents Sign in with ChatGPT for Codex clients, not as a general OpenAI Platform OAuth contract for arbitrary third-party applications. The existing OpenAI API-key adapter remains the stable path.

### xAI / Grok

Adapt the OIDC discovery, device authorization, bounded polling, refresh, and error classification patterns from `../tui`. Endpoints and client identifiers are configuration/capability inputs rather than scattered constants. The xAI API-key adapter remains available and the direct subscription path remains visibly experimental.

### Gemini

Use the Gemini API's documented OAuth Bearer-token path and the native Gemini provider adapter. Forge ships or is configured with its own registered Google OAuth client and consent metadata. It never reads Gemini CLI storage, uses Gemini CLI client identifiers, or sends Gemini CLI/Code Assist credentials to its backend. AI Studio API keys remain the simple fallback.

## Security and Failure Handling

- Authorization operations are account-owned, short-lived, rate-limited, and finite-state.
- State and PKCE validation are mandatory; provider callback errors are redacted and safe to render.
- Device codes and PKCE verifiers remain server-side and are erased after terminal completion.
- Access tokens, refresh tokens, API keys, authorization codes, and client secrets are excluded from logs, events, traces, URLs controlled by Forge, and database metadata columns.
- OAuth scopes use the provider's minimum supported inference/profile set; unrelated account data is not requested.
- Refresh, profile publication, activation, and binding changes retain optimistic concurrency and never leave a partially active profile.
- Provider outages affect only the relevant connection. Existing chats, Tasks, and Project data remain readable.
- Login does not grant new Forge tools, Project scope, or repository authority; authority continues to come from server-issued role/scope policy.

## Alternatives Rejected

- Keeping global/project chat switching inside the chat page: it duplicates the application hierarchy and makes the Main Agent appear Project-local.
- Giving Project Agent chat repository editing tools: this bypasses Task assignment, Workspace leases, evidence, review, and release boundaries.
- Keeping `/agents`, `/agents/federated`, and Project Agent settings as separate concepts: it forces users to understand backend implementation boundaries to configure one agent.
- Importing existing CLI credentials: it couples Forge to another application's storage/security decisions and makes revocation ownership ambiguous.
- Using Gemini CLI OAuth: Google's published terms explicitly prohibit the third-party access model.
- Removing API keys: not every provider or deployment supports an appropriate interactive OAuth flow.

## Rollout and Validation

There is no feature flag. Implementation updates root `DESIGN.md` before components, then removes old navigation/settings surfaces in the same change. Data migrations are forward-only and preserving.

Validation includes Rust unit/integration tests, the canonical API happy path, web lint/typecheck/tests/build, provider mock servers for browser/device/refresh/error paths, credential redaction tests, migration tests from existing API-key data, accessibility checks, and live browser proof at 375 px, 768 px, and 1280 px widths. Real-provider smoke tests use test accounts and never commit or print credentials.

## Dependency and Merge Order

`add-project-agent-federation-2026-08-12` is the implemented baseline but has not yet been archived into source-of-truth specs. Before this change is archived, its `Chat Switcher and Global Launcher` requirement must be merged first and then replaced by this change's full modified requirement. The older requirement must never be archived after this one in a way that restores the nested chat switcher.
