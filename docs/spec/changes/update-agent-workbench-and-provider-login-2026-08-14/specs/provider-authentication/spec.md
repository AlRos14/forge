## ADDED Requirements

### Requirement: Provider Authentication Capability Catalog

Forge SHALL expose a server-owned catalog for OpenAI, xAI/Grok, Gemini, OpenRouter, and OpenAI-compatible providers. Each entry SHALL declare supported credential methods, configuration readiness, model-discovery capability, runtime compatibility per credential method (`direct` and harness kinds) with per-combination support levels, and a user-visible stable, experimental, or unavailable support level. The web client SHALL render available connection and runtime actions from this catalog rather than hard-coded provider assumptions.

#### Scenario: User opens provider connection

- **WHEN** the user opens the provider connection flow
- **THEN** Forge displays only methods declared by the server for each provider
- **AND** experimental methods and missing OAuth client configuration are labeled before authorization begins

#### Scenario: API-key fallback is available

- **WHEN** a provider has an interactive login method and an API-key method
- **THEN** the user may explicitly choose either supported method
- **AND** selecting one does not overwrite an existing credential until verification and provider-entry publication succeed

### Requirement: Finite Provider Authorization Operations

Forge SHALL execute browser and device authorization as short-lived, account-owned, finite-state operations. The server SHALL own PKCE verifiers, device secrets, token exchange, provider polling, and cancellation. Public operation responses SHALL contain only an opaque operation ID, public authorization URL or user code, expiry/polling guidance, redacted state, and user-safe errors.

#### Scenario: Browser authorization succeeds

- **WHEN** the initiating account completes a browser OAuth callback with valid state and PKCE evidence before expiry
- **THEN** Forge exchanges and protects the credential, verifies provider access, and marks the operation succeeded
- **AND** no authorization code or token is returned to the web client or written to logs/events

#### Scenario: Device authorization is pending then succeeds

- **WHEN** a user completes a supported device flow while Forge polls at the provider-prescribed bounded interval
- **THEN** Forge transitions the operation from pending to succeeded exactly once
- **AND** provider `slow_down` responses increase polling delay rather than creating parallel polling loops

#### Scenario: Authorization terminates without connection

- **WHEN** an operation is denied, cancelled, expired, malformed, replayed, or exhausts its finite retry budget
- **THEN** Forge records a redacted terminal outcome and erases transient secret material
- **AND** no provider entry or agent profile is published and no active binding is changed

#### Scenario: Another account reads an operation

- **WHEN** an authenticated user requests an authorization operation owned by another account
- **THEN** Forge denies the request without disclosing provider, code, URL, account, or failure details

### Requirement: Protected Renewable Provider Credentials

Forge SHALL store API keys and versioned OAuth bundles only through the encrypted protected-credential store. OAuth bundles SHALL support access token, refresh token, expiry, scopes, provider account identity, and schema version. Secrets SHALL be excluded from public APIs, logs, events, traces, debug output, and unencrypted metadata.

#### Scenario: Access token requires refresh

- **WHEN** concurrent requests encounter an expired or near-expiry access token
- **THEN** Forge performs at most one refresh per protected credential and shares the resulting current bundle
- **AND** refresh-token rotation and access-token replacement commit atomically with optimistic concurrency

#### Scenario: Authentication fails before output

- **WHEN** a provider rejects authorization before any assistant output has streamed
- **THEN** Forge may refresh and retry the provider request at most once
- **AND** it surfaces a redacted connection failure if the retry fails

#### Scenario: Authentication fails after output starts

- **WHEN** a provider failure occurs after assistant output has started
- **THEN** Forge does not replay the semantic request
- **AND** it records an explicit partial/failed turn outcome without duplicating output

#### Scenario: Existing API-key data is migrated

- **WHEN** an existing installation applies the provider-authentication migration
- **THEN** every existing protected credential remains usable and is classified as an API-key credential
- **AND** no historical migration is edited and no credential is exposed or dropped

### Requirement: OpenAI Login Support Boundary

Forge SHALL offer `Continue with ChatGPT` through browser PKCE with a supported device-code fallback and SHALL use renewable credentials for the direct ChatGPT adapter. Forge SHALL label direct ChatGPT subscription access experimental and SHALL retain OpenAI Platform API-key connection as the stable alternative. Forge SHALL NOT import Codex or TUI credential storage.

#### Scenario: User chooses ChatGPT login

- **WHEN** an authorized user starts `Continue with ChatGPT`
- **THEN** Forge begins the declared browser or device authorization operation and explains the experimental direct-backend boundary
- **AND** successful verification publishes a Forge-owned OpenAI provider entry without reading another application's auth cache

#### Scenario: Experimental ChatGPT adapter is unavailable

- **WHEN** the direct ChatGPT flow is unsupported, disabled, or rejected by the provider
- **THEN** Forge leaves existing provider entries, agents, and bindings unchanged and presents a redacted recovery action
- **AND** the OpenAI Platform API-key method remains available

### Requirement: xAI Grok Login Support Boundary

Forge SHALL offer xAI device authorization using discovered OAuth 2.0/OIDC endpoints, provider-prescribed polling, and renewable credentials. Forge SHALL label subscription-backed direct Grok access experimental and SHALL retain xAI API-key connection as an alternative.

#### Scenario: User chooses xAI device login

- **WHEN** an authorized user starts xAI login
- **THEN** Forge presents the provider verification URL, public user code, expiry, and bounded status updates
- **AND** successful verification publishes an xAI/Grok provider entry with its protected credential reference

#### Scenario: xAI discovery or authorization fails

- **WHEN** discovery metadata is invalid or the device operation fails terminally
- **THEN** Forge reports a redacted provider-specific recovery action without publishing a provider entry
- **AND** the xAI API-key method and existing connections remain unchanged

### Requirement: Gemini Supported OAuth Boundary

Forge SHALL offer Google authorization only for the documented Gemini API through a registered Forge OAuth client and SHALL retain the Gemini AI Studio API-key method. Forge SHALL NOT import Gemini CLI credentials, use Gemini CLI/Code Assist client identity, or access the services powering Gemini CLI through Gemini CLI OAuth.

#### Scenario: Registered Gemini OAuth client is available

- **WHEN** an authorized user chooses `Continue with Google` and Forge has valid registered OAuth client configuration
- **THEN** Forge uses the documented Gemini API OAuth flow and verifies the resulting API access before publishing a Gemini provider entry
- **AND** the credential is scoped and stored as a Forge-owned protected OAuth bundle

#### Scenario: Gemini OAuth client is not configured

- **WHEN** a development or self-hosted Forge build lacks registered Google OAuth client configuration
- **THEN** the capability catalog reports Google login unavailable with actionable setup guidance
- **AND** Gemini AI Studio API-key connection remains available

#### Scenario: Client attempts Gemini CLI credential reuse

- **WHEN** a client attempts to submit, import, or reference Gemini CLI/Code Assist OAuth credentials
- **THEN** Forge rejects the operation without storing or forwarding the credential
- **AND** it directs the user to Gemini API OAuth or an AI Studio API key

### Requirement: Transactional Provider Entry Publication and Disconnect

Forge SHALL publish or update a provider entry only after authorization, credential protection, provider verification, and required discovery succeed; immutable agent profiles SHALL be published only by explicit agent creation referencing an entry. Disconnect SHALL make the entry unusable transactionally, clear protected secret material, attempt provider revocation when supported, preserve non-secret audit provenance, and surface affected agents and bindings without silently reassigning them.

#### Scenario: Connection setup fails after token exchange

- **WHEN** token exchange succeeds but provider verification or entry publication fails
- **THEN** Forge removes or invalidates the uncommitted protected credential and reports a redacted failure
- **AND** prior provider entries, agents, and bindings remain unchanged

#### Scenario: User disconnects a bound connection

- **WHEN** an authorized user confirms disconnect for a credential used by a Main or Project binding
- **THEN** Forge makes the credential unavailable, clears its protected secret, and visibly marks each affected binding as requiring recovery
- **AND** it does not silently select a different agent or credential

#### Scenario: Provider revocation endpoint fails

- **WHEN** local disconnect commits but provider-side revocation fails
- **THEN** Forge keeps the local credential unusable and reports a redacted best-effort revocation warning
- **AND** secret material is not restored to retry revocation automatically
