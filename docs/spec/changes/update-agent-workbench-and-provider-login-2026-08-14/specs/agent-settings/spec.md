## ADDED Requirements

### Requirement: Canonical Agent Settings Surface

Forge SHALL provide one account-level `Agent Settings` surface at `/agents` for every available agent identity/profile, CLI/runtime agent, provider connection, credential method, health state, and authorized Main/Project/Task role. It SHALL NOT require users to configure the same agent through separate federated-agent or Project-agent settings destinations.

#### Scenario: User inventories all available agents

- **WHEN** a user opens Agent Settings
- **THEN** Forge shows every agent and profile the user is authorized to view with kind, provider/model, connection health, credential method, and current roles
- **AND** the inventory can be searched and filtered without changing server authority

#### Scenario: User completes an agent setup workflow

- **WHEN** a user connects, reconnects, disconnects, activates a profile, or changes an authorized Main or Project binding
- **THEN** the interaction completes within the canonical Agent Settings surface
- **AND** its success, pending state, or failure is visible without navigating to a duplicate configuration page

#### Scenario: Project context links to canonical settings

- **WHEN** a Project surface needs Project Agent setup or recovery
- **THEN** its action links to `/agents` with an optional Project context filter
- **AND** the filter does not grant Project access or binding authority

#### Scenario: Duplicate settings destinations are removed

- **WHEN** the new Agent Settings surface ships
- **THEN** `/agents/federated` and the Project-local `project-agent` settings tab are removed rather than retained as aliases
- **AND** all first-party call sites target the canonical surface

### Requirement: Provider Entries Separate From Agents

Agent Settings SHALL present configured provider entries and runnable agents as two tabs of one settings model. A provider entry SHALL represent one credentialed connection with provider type, credential method, editable display name, discovered account identity, health, and usage. Users SHALL be able to configure multiple entries of the same provider type, and completing a provider connection SHALL NOT create an agent.

#### Scenario: User adds a second entry of the same provider type

- **WHEN** a user with an existing OpenAI provider entry adds another OpenAI entry with a different account or credential
- **THEN** both entries appear independently on the Providers tab with distinguishable display names, independent health, and independent usage
- **AND** neither entry's credential or health affects the other

#### Scenario: Completing a provider connection creates no agent

- **WHEN** a guided login or API-key submission completes successfully
- **THEN** the Providers tab shows the new entry as connected and the Agents roster is unchanged
- **AND** the success state offers agent creation referencing the new entry as an explicit follow-up action

#### Scenario: Provider entry shows usage

- **WHEN** a user views a provider entry
- **THEN** the entry shows which agents reference it and when it was last used
- **AND** the usage display links to the Agents tab filtered to that entry

#### Scenario: Removing a referenced provider entry

- **WHEN** a user removes a provider entry that one or more agents reference
- **THEN** Forge warns with the dependent agents before removal
- **AND** after removal the dependent agents are visibly unhealthy rather than silently rebound or deleted

### Requirement: CLI Runtime Visibility

The Providers tab SHALL display discovered CLI-managed runtimes alongside provider entries, showing CLI authentication health, host runtime, and usage, without importing or reading another application's credential files.

#### Scenario: CLI runtime appears with health and usage

- **WHEN** a runtime host reports an installed CLI harness
- **THEN** the Providers tab lists it with its authentication state, host, and the agents that use it
- **AND** an unauthenticated CLI shows recovery guidance naming the login command and host instead of an editable credential form

### Requirement: Guided Agent Registration

Agent creation SHALL be a guided flow that references exactly one authentication source — a provider entry or a CLI-managed runtime — and one runtime kind (`direct` or a harness), with runtime compatibility supplied by the server capability catalog rather than inferred by the client.

#### Scenario: User creates an agent from a provider entry

- **WHEN** a user starts agent creation, selects a provider entry, selects a compatible runtime kind, and submits configuration
- **THEN** Forge publishes the immutable profile referencing that entry and runtime
- **AND** the new agent appears in the roster without changing any Main or Project binding

#### Scenario: Incompatible runtime is rejected

- **WHEN** the selected provider entry does not support a runtime kind
- **THEN** the client presents that runtime option as unavailable with a user-safe reason from the capability catalog
- **AND** the server rejects the incompatible combination if submitted directly

#### Scenario: No authentication source exists at wizard start

- **WHEN** a user starts agent creation with no configured authentication source
- **THEN** the flow offers adding a provider inline
- **AND** completing that connection returns the user to the creation flow with the new entry selected

### Requirement: Agent Settings Authorization Separation

Agent Settings SHALL enforce account ownership, Project authorization, binding cardinality, immutable profile history, and optimistic concurrency at the server. Forge-wide runtime defaults SHALL remain admin-only under Forge Settings and SHALL be visually distinguished from account-owned agent configuration.

#### Scenario: Non-admin views Agent Settings

- **WHEN** an authenticated non-admin opens Agent Settings
- **THEN** Forge returns only agent records and actions authorized for that account and its accessible Projects
- **AND** server/runtime/path defaults are not exposed as editable account agent settings

#### Scenario: Stale binding change is submitted

- **WHEN** a client submits a Main or Project binding update against an obsolete version
- **THEN** Forge returns a version conflict without changing the active binding
- **AND** the UI offers the current state for reconciliation
