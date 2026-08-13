## ADDED Requirements

### Requirement: Bounded Mission Control Home

Forge SHALL provide a read-only Mission Control home reporting needs attention, review-ready work, active work, Main Agent health, exactly one Project Agent health entry per operational Project, relevant Task Worker/reviewer activity, recent outcomes, commitments, and bounded runtime capacity. It SHALL derive sections from authoritative records/projections rather than duplicate Task, Chat, or Agent truth.

#### Scenario: Home reports current priorities accurately

- **WHEN** authorized scopes contain Attention, review-ready Tasks, running work, Main/Project agents, relevant Task agents, and recent outcomes
- **THEN** each entity appears once in the correct bounded section with Project context and primary action

#### Scenario: Configured profiles do not flood the roster

- **WHEN** many connected or default profiles exist without active Main/Project binding or Task work
- **THEN** Mission Control does not render each one as a persistent conversational agent
- **AND** identity inventory remains available in Agent management

#### Scenario: Cross-project authorization is enforced

- **WHEN** a user cannot access one Project represented in global projections
- **THEN** that Project's binding, Tasks, counts, health, chat, and Attention do not affect the response

#### Scenario: Routine internals stay secondary

- **WHEN** the system is healthy and work needs no intervention
- **THEN** Mission Control does not default to terminal streams, workspace paths, daemon IDs, raw runtime events, or thousands of pixels of idle agent cards

### Requirement: Binding-Centered Agent Detail and Continuity Health

Forge SHALL provide authorized Agent detail containing stable identity/current profile, Main/Project bindings, Task roles, active/suspended canonical scopes/sessions/capabilities, current focus, inbox/commitments, memory namespaces/access, subscriptions/wake budget, usage, and continuity indicators. Protected state and inaccessible memory bodies SHALL never be returned.

#### Scenario: Inspect Main Agent

- **WHEN** an authorized user opens the Main Agent detail
- **THEN** the response identifies its global binding, current profile/session, focus, commitments, capabilities, and global-chat continuity
- **AND** it does not fabricate Task or repository authority

#### Scenario: Inspect Project Agent

- **WHEN** an authorized user opens a Project Agent detail
- **THEN** the response identifies the unique Project binding, Project Chat, current focus, commitments, Task-management policy, and continuity health
- **AND** historical replacement attribution remains inspectable

#### Scenario: Inspect connected unbound identity

- **WHEN** an authorized user opens an identity with no Main/Project binding or active Task role
- **THEN** Forge shows its connection/profile health and available binding actions
- **AND** it does not fabricate a persistent chat or Project role

#### Scenario: Backend is unhealthy but identity remains durable

- **WHEN** a backend session is offline while identity/bindings/commitments remain durable
- **THEN** the view distinguishes backend health from identity and obligation state
- **AND** exposes only policy-allowed restart/replace actions

### Requirement: Chat Switcher Read Model

Forge SHALL provide a bounded authorized read model for the left chat switcher containing the global Main Chat and one Project Agent Chat per authorized Project, including binding/setup state, unread/turn status, relevant Attention, and stable navigation identity.

#### Scenario: Project Agent turn is retrying

- **WHEN** a Project Chat has a turn in `retry_wait`
- **THEN** the switcher entry exposes a bounded retrying indicator and Project identity
- **AND** it does not appear healthy or completed merely because the user message committed

#### Scenario: Project needs Agent setup

- **WHEN** a migrated Project is `agent_setup_required`
- **THEN** its switcher entry exposes setup state and primary setup action
- **AND** no arbitrary responder identity is displayed

### Requirement: Live Projection Updates

Mission Control, Agent detail, and chat-switcher clients SHALL update from committed domain-event projections using bounded invalidation/refetch. A missed browser or in-process event SHALL recover from authoritative read APIs and SHALL NOT require the client to reconstruct state from event payload history.

#### Scenario: Attention resolves while home is open

- **WHEN** a displayed Attention condition resolves
- **THEN** the client refetches the affected bounded section and removes/updates the item

#### Scenario: Chat turn changes while switcher is visible

- **WHEN** a queued turn becomes retrying, failed, or succeeded
- **THEN** the client invalidates/refetches the chat and switcher read models
- **AND** both converge on the same authoritative state

#### Scenario: Client reconnects after event gap

- **WHEN** the client reconnects after missing live events
- **THEN** it reloads authoritative Mission Control, Agent detail, and switcher read models
- **AND** displayed state converges without replaying side effects
