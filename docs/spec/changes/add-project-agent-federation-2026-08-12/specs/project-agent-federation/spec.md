## ADDED Requirements

### Requirement: One Global Main Agent

Forge SHALL expose one account-level Main Agent binding and one durable global Agent Chat. The binding SHALL select a stable identity and immutable profile revision under optimistic concurrency; replacing the binding SHALL preserve the chat and all historical provenance.

#### Scenario: Account has no Main Agent yet

- **WHEN** an authorized account has not selected a Main Agent
- **THEN** Forge returns an explicit Main Agent setup state and the global chat admits no model turn
- **AND** it does not silently select any connected identity

#### Scenario: Concurrent Main Agent replacement

- **WHEN** two callers replace the Main Agent from the same expected version
- **THEN** exactly one replacement commits
- **AND** the loser receives a version conflict without changing the global chat

### Requirement: Exactly One Operational Project Agent

Every newly created operational Project SHALL have exactly one active Project Agent binding. The invariant SHALL be enforced independently of Task roles. A migrated Project with no safely inferable identity MAY enter explicit `agent_setup_required` state, but it SHALL admit no Project Agent turns until exactly one binding is selected.

#### Scenario: Project creation is atomic

- **WHEN** an authorized actor creates a Project with an eligible Project Agent selection
- **THEN** the Project, unique binding, Project Agent Chat, and matching events commit atomically
- **AND** the Project is immediately ready for Project Agent turns

#### Scenario: Project is created before Agent selection

- **WHEN** an authorized human or existing automation creates a Project without selecting a Project Agent
- **THEN** Forge creates the Project and its singular Project Chat in explicit `agent_setup_required` state
- **AND** the Project remains non-operational for Project Agent turns until exactly one eligible binding is selected
- **AND** existing human and Task workflow APIs remain usable without Forge fabricating an agent identity

#### Scenario: Ambiguous migrated Project

- **WHEN** migration finds zero or multiple plausible Project Agent identities
- **THEN** it preserves all Project, Task, chat, and provenance data and marks the Project `agent_setup_required`
- **AND** it does not infer authority from a primary Worker or arbitrary recent responder

#### Scenario: Concurrent Project Agent assignment

- **WHEN** concurrent callers attempt to establish different active Project Agents for one Project
- **THEN** the database and version check permit exactly one active binding
- **AND** no role-dependent predicate permits a second primary manager

### Requirement: Explicit Main-to-Project Handoff

The Main Agent SHALL communicate discovery context to a Project Agent only through an immutable, typed handoff targeted to an authorized Project. Forge SHALL persist the handoff's source and target provenance, publish a visible bounded message in the Project Agent Chat, and admit at most one Project Agent turn.

#### Scenario: Handoff after Project creation

- **WHEN** the Main Agent publishes an authorized Project brief with stable source references
- **THEN** Forge appends one Project Chat handoff message and one turn job transactionally
- **AND** the user receives a link/action to continue in that Project Agent Chat

#### Scenario: Repeat handoff is an explicit update

- **WHEN** the Main Agent sends later approved context to the same Project
- **THEN** Forge records another attributed handoff rather than mutating the original
- **AND** the Project Agent can distinguish the update and its source revisions

#### Scenario: Handoff does not propagate authority

- **WHEN** a handoff contains references to global memory, another Project, a Task, credentials, or requested tools
- **THEN** Forge includes only authorized bounded content and safe reference metadata
- **AND** the target Project Agent receives no Main Agent, cross-project, protected-state, or repository authority

#### Scenario: Handoff replay is idempotent

- **WHEN** the same deduplication key is replayed after timeout or restart
- **THEN** Forge returns the original handoff/turn outcome
- **AND** it creates no duplicate Project Chat message or agent turn

### Requirement: Project Agent Owns Project Task Management

Only the active Project Agent binding SHALL give a persistent core agent authority to propose and manage Tasks within that Project. Forge SHALL validate every mutating proposal against the binding, canonical Project scope, action policy, Task contract, budget, and deduplication key before invoking existing Task services.

#### Scenario: Project Agent creates planned work

- **WHEN** the Project Agent proposes a valid Task in its own Project
- **THEN** one authoritative Task is persisted through the normal Task service
- **AND** Task execution remains delegated to assigned Workers/reviewers through the existing workflow

#### Scenario: Main Agent is denied Task management

- **WHEN** the Main Agent requests Task creation, mutation, transition, assignment, review, or merge
- **THEN** Forge records a typed denial and changes no Task

#### Scenario: Project Agent replacement preserves obligations

- **WHEN** a Project Agent binding is replaced
- **THEN** existing Project commitments and inbox outcomes remain durably attributed to their original identity
- **AND** an authorized transfer is required before the replacement owns an unfinished commitment

### Requirement: Bounded Global Portfolio Management

The Main Agent SHALL receive authorized bounded portfolio projections sufficient to create, list, organize, and summarize Projects without receiving implicit Project Agent Chat history, Project-private memory, Task mutation capability, or repository access.

#### Scenario: Summarize authorized Projects

- **WHEN** the Main Agent requests portfolio status
- **THEN** Forge returns bounded Project identity, lifecycle, health, attention, and aggregate work summaries for authorized Projects
- **AND** inaccessible Projects and private chat/memory content do not affect results or counts

#### Scenario: Attach current Project reference from global launcher

- **WHEN** the user explicitly attaches an authorized Project reference to a Main Chat message
- **THEN** Forge may offer the bounded portfolio projection for that Project
- **AND** opening the launcher alone does not import the Project Chat timeline or Project-private memory
