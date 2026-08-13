## ADDED Requirements

### Requirement: Direct Forge Agent Connection

Forge SHALL let an authorized user create an account-owned `AgentIdentity` and immutable executable `AgentProfile` without first creating a Project. A native profile SHALL select a supported provider, model, tool policy, and opaque protected credential reference, and Forge SHALL compose it directly over the pinned Agent Runtime without requiring Smith, TUI, or a separate host process.

#### Scenario: Connect an unbound agent

- **WHEN** an authorized user creates an identity and valid native profile with no Main Agent, Project Agent, or Task binding
- **THEN** Forge persists the stable identity and immutable profile revision
- **AND** the identity is available for later binding or Task assignment without creating an independent durable chat

#### Scenario: Validate provider connectivity safely

- **WHEN** the user submits a provider credential through the protected connection flow
- **THEN** Forge returns bounded provider/model health and capability status
- **AND** no raw credential appears in a profile, chat, memory, manifest, event, log, or public response

#### Scenario: Invalid connection grants no authority

- **WHEN** provider/model validation fails
- **THEN** Forge records a redaction-safe failure and starts no native session
- **AND** the failed connection creates no Project, chat, Task, tool, or filesystem authority

### Requirement: Singular Main and Project Bindings

Forge SHALL model the global Main Agent and each Project Agent as explicit versioned bindings of account-owned identities, not as identity types or role/primary combinations. An account SHALL have at most one active Main Agent binding, and each operational Project SHALL have exactly one active Project Agent binding.

#### Scenario: Select the Main Agent

- **WHEN** an authorized user binds an eligible identity/profile as Main Agent
- **THEN** that binding becomes the only active Main Agent binding for the account
- **AND** the existing global Agent Chat remains the canonical global timeline

#### Scenario: Create an operational Project

- **WHEN** Forge creates a new operational Project
- **THEN** it atomically creates exactly one active Project Agent binding and one Project Agent Chat
- **AND** failure of any part commits none of the Project, binding, or chat

#### Scenario: Replace a Project Agent

- **WHEN** an authorized user replaces a Project Agent with another eligible identity/profile using the expected version
- **THEN** future Project Chat turns use the new binding
- **AND** historical messages, handoffs, commitments, profiles, sessions, and attribution remain unchanged

#### Scenario: Task role cannot become Project Agent implicitly

- **WHEN** an identity is assigned as a Task Worker or reviewer
- **THEN** that assignment does not create or replace a Project Agent binding
- **AND** no `role` or `is_primary` value can bypass the unique Project Agent invariant

### Requirement: Explicit Canonical Agent Scope

Every embedded session SHALL bind exactly one server-authorized canonical scope: a Main Agent Chat, Project Agent Chat, or Task. Identity/profile selection SHALL NOT itself grant access; effective context, memory, tools, timeline, and workspace SHALL derive from the active binding or Task assignment for that exact scope.

#### Scenario: Main scope is not omniscient

- **WHEN** the Main Agent starts a global-chat session
- **THEN** it receives only account policy, bounded authorized portfolio projections, global chat context, and global memory
- **AND** it receives no implicit Project Chat history, Project-private memory, Task mutation tools, or repository filesystem

#### Scenario: One identity serves several scopes

- **WHEN** the same identity is Main Agent, Project Agent for one or more Projects, or an assigned Task agent
- **THEN** Forge creates/resumes distinct scope-bound sessions and timelines
- **AND** permissions, context, memory, tools, and workspace from one scope do not leak into another

#### Scenario: Binding or assignment is revoked

- **WHEN** the grant authorizing an active scope is replaced, removed, paused, or expires
- **THEN** Forge admits no new work under that grant
- **AND** in-flight continuation or cancellation follows bounded backend policy without widening another scope

### Requirement: Main Agent Authority Boundary

The Main Agent SHALL support discovery, configured web search, account-level Project organization, bounded portfolio summaries, and explicit handoff. Its server-issued tools SHALL NOT permit Task creation, editing, assignment, transition, review, merge, delivery, or repository mutation.

#### Scenario: Main Agent creates and hands off a Project

- **WHEN** the Main Agent submits authorized Project metadata, an eligible Project Agent selection, and a bounded handoff
- **THEN** Forge creates the Project/binding/chat through account policy and publishes the explicit handoff
- **AND** the Project Agent receives one admitted Project Chat turn

#### Scenario: Main Agent attempts Task creation

- **WHEN** Main Agent output or a caller reference requests creation or mutation of a Task
- **THEN** Forge returns a typed policy denial and creates no Task action
- **AND** prompt text, memory, handoff content, or Project references cannot widen that denial

### Requirement: Project Agent Task Management Boundary

A Project Agent SHALL create and manage Tasks only for its bound Project through typed action envelopes and the existing Task service, workflow, permission, budget, validation, review, and delivery paths. The Project Agent Chat SHALL receive no repository Workspace.

#### Scenario: Project Agent proposes a Task

- **WHEN** the active Project Agent submits a valid deduplicated Task action for its Project
- **THEN** Forge validates policy and persists the authoritative Task through `TaskService`
- **AND** implementation occurs only after normal Task assignment/claim/workflow admission

#### Scenario: Project Agent targets another Project

- **WHEN** a Project Agent action targets a different Project
- **THEN** Forge denies it before target data or mutation is exposed

#### Scenario: Project Agent attempts repository mutation

- **WHEN** a Project Agent Chat turn requests filesystem or repository tools
- **THEN** the session retains a deny-all filesystem workspace
- **AND** code mutation remains confined to an admitted Task Worker Workspace

### Requirement: Embedded Task Worker and Reviewer

Forge SHALL allow a compatible embedded profile to serve as a Task Worker or reviewer only through existing Task role assignment, claim, workflow, Workspace, validation, review, and delivery paths. Task scope SHALL be separate from Main and Project Chat authority.

#### Scenario: Embedded Worker claims a Task

- **WHEN** an embedded identity is assigned a compatible Worker role and claims an eligible Task
- **THEN** Forge starts or resumes a Task-scoped session with only that Task's authorized Workspace/tools
- **AND** existing workflow, validation, review, version-conflict, and delivery rules remain authoritative

#### Scenario: Reviewer receives review authority only

- **WHEN** an embedded identity is assigned as reviewer
- **THEN** its Task-scoped session receives only review-policy capabilities
- **AND** it cannot acquire Worker, merge, Main Agent, Project Agent, or unrelated Project authority

#### Scenario: Task agent does not become a chat destination

- **WHEN** an identity is used only as a Task Worker or reviewer
- **THEN** it does not appear as another persistent Agent Chat in the left switcher
- **AND** its Task timeline remains inspectable through Task evidence surfaces
