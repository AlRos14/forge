## ADDED Requirements

### Requirement: Durable Evidence-Backed Commitment

Forge SHALL persist commitments independently of bindings, profiles, and backend sessions, including owner identity, canonical scope, status, due date, correlation, evidence references, and transfer/cancellation reason. A commitment SHALL NOT become completed without authorized evidence, and delivery of a request or handoff alone SHALL NOT imply completion.

#### Scenario: Commitment survives session or binding failure

- **WHEN** an embedded agent accepts a commitment and its backend session fails or its binding is later replaced
- **THEN** the commitment remains attributed to the original identity with the same status and evidence requirements

#### Scenario: Reject completion without evidence

- **WHEN** an agent requests completion with no authorized evidence reference
- **THEN** Forge rejects the transition and leaves the commitment open

#### Scenario: Transfer with reason

- **WHEN** an authorized actor transfers a commitment to a replacement Project Agent
- **THEN** Forge records the new owner and reason
- **AND** original ownership history remains auditable

#### Scenario: Handoff creates no implicit completion

- **WHEN** the Main Agent successfully delivers a handoff to a Project Agent
- **THEN** only the handoff-delivery obligation may be evidenced as satisfied
- **AND** Project setup, Task, or product outcomes remain open until separately evidenced

### Requirement: Typed Agent Action and Policy Result

Every mutating embedded-agent tool call SHALL create or bind to a typed action/proposal envelope containing actor identity, canonical scope, operation, payload hash, deduplication key, correlation/causation, requested permission, and policy result. Forge SHALL be the only component that authorizes and persists authoritative mutations.

#### Scenario: Allowed proposal executes once

- **WHEN** an authorized agent submits the same valid action and deduplication key more than once
- **THEN** Forge records one effective execution and returns the original outcome on replay

#### Scenario: Denied proposal changes nothing

- **WHEN** effective permission denies an action
- **THEN** Forge records a bounded denial policy result
- **AND** no authoritative target mutation occurs

#### Scenario: Agent cannot self-approve

- **WHEN** a protected action requires approval and the proposing identity attempts to approve it
- **THEN** Forge denies the approval and preserves the pending action

### Requirement: Scope-Specific Action Catalogs

Forge SHALL derive mutating tools from canonical scope. The Main Agent action catalog SHALL contain only authorized account Project-lifecycle, organization, and handoff actions. The Project Agent catalog SHALL contain only actions for its bound Project, including Task management. Task execution catalogs SHALL remain assignment/workflow-specific.

#### Scenario: Main Agent requests Task mutation

- **WHEN** the Main Agent submits or fabricates a Task action envelope
- **THEN** Forge denies it because the operation is absent from Main scope
- **AND** supplying a Project/Task ID does not change the result

#### Scenario: Project Agent proposes own-Project Task

- **WHEN** the active Project Agent proposes a valid Task within its bound Project
- **THEN** Forge records policy and creates one authoritative Task through `TaskService`
- **AND** no repository write occurs in the Project Agent Chat session

#### Scenario: Project Agent targets another Project

- **WHEN** the Project Agent proposes any mutation against a different Project
- **THEN** Forge denies it before target existence or content is disclosed

### Requirement: Explicit Handoff Action

Main-to-Project communication SHALL use a typed handoff action bound to the Main Chat and target Project. Forge SHALL guard, authorize, deduplicate, persist, and deliver the bounded publication before a Project Agent turn is admitted.

#### Scenario: Valid handoff executes once

- **WHEN** the Main Agent submits an authorized handoff with stable source revisions
- **THEN** Forge creates one handoff, one Project Chat message, and at most one target turn
- **AND** the source Main Chat receives one durable delivery receipt

#### Scenario: Handoff contains unauthorized content

- **WHEN** the proposed publication includes protected or inaccessible memory bodies
- **THEN** Forge denies or safely omits them according to policy before delivery
- **AND** no target message, memory record, event, or log leaks the content

### Requirement: Idempotent Task Delivery Reconciliation

Forge SHALL reconcile Task delivery, failure, and cancellation evidence to the originating Project Agent inbox and commitment through a durable idempotent event consumer. Replay SHALL NOT duplicate inbox messages, evidence links, or commitment transitions.

#### Scenario: Delivery closes originating commitment once

- **WHEN** a Project-Agent-originated Task produces accepted evidence satisfying its commitment
- **THEN** evidence is attached and the commitment completes exactly once
- **AND** the originating identity receives one outcome inbox item even if its binding changed

#### Scenario: Failed delivery leaves explicit obligation state

- **WHEN** a Task is blocked, failed, or cancelled without satisfying the commitment
- **THEN** reconciliation records the outcome once
- **AND** the commitment remains open, blocked, transferred, or cancelled with an explicit reason
