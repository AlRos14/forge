## ADDED Requirements

### Requirement: Transactional Domain Event Ledger

Agent-critical authoritative mutations SHALL write a monotonic, scoped, typed domain event in the same database transaction. A binding, Agent Chat message/turn, handoff, session, memory publication, commitment, action, or relevant Task mutation and its event SHALL NOT commit independently. The in-process broadcast bus SHALL NOT be authoritative.

#### Scenario: Mutation and event commit together

- **WHEN** Forge commits an agent-critical mutation
- **THEN** the matching domain event is durably visible at commit
- **AND** transaction failure leaves neither mutation nor event visible

#### Scenario: Broadcast delivery is lost

- **WHEN** the process crashes after database commit but before in-process broadcast is observed
- **THEN** consumers recover the event from the durable sequence after restart

### Requirement: Replayable Idempotent Consumers

Each projection, chat-turn, handoff, indexing, reconciliation, and wake consumer SHALL persist cursor and idempotency state so replay cannot duplicate Attention items, turn jobs, handoffs, agent actions, inbox outcomes, or commitment updates.

#### Scenario: Consumer resumes after lag

- **WHEN** a consumer falls behind or restarts
- **THEN** it resumes from its durable cursor and processes subsequent committed events in order

#### Scenario: Replay prior events

- **WHEN** a consumer replays a range containing already-applied events
- **THEN** projections and actions equal the pre-replay state
- **AND** no duplicate user or agent work is admitted

### Requirement: Finite Chat Turn Recovery

Turn recovery SHALL be deterministic and bounded before any model invocation. It SHALL enforce one active lease per chat, lease expiry, finite attempts, cooldown/backoff, deduplication, current binding/scope authorization, and terminal failure. Failure to persist an execution outcome SHALL NOT create an unbounded silent retry loop.

#### Scenario: Expired chat lease is recovered

- **WHEN** a leased turn has no terminal outcome and its lease expires
- **THEN** a deterministic consumer moves it to the next allowed retry or terminal failure using optimistic concurrency
- **AND** restart does not reset attempts or duplicate a response

#### Scenario: Failure state initially contends

- **WHEN** the worker cannot commit failure state because SQLite returns a transient persistence conflict
- **THEN** the lease remains finite and recovery retries only the state transition or admitted turn according to stored policy
- **AND** the job eventually becomes visible `retry_wait` or `failed`

#### Scenario: Retry budget exhausted

- **WHEN** a turn reaches its finite maximum attempts
- **THEN** Forge persists one terminal failure event and creates/updates one actionable Attention item
- **AND** no automatic model wake continues for that turn

### Requirement: Deterministic Attention Before Model Wake

Known conditions SHALL use deterministic rules before model invocation. Wake admission SHALL enforce deduplication, batching/cooldown, applicable account/binding/Project/Task budgets, correlation/causation, maximum reaction depth, self-event suppression, unchanged-state suppression, and at most one active analysis lease per incident.

#### Scenario: Known transient condition uses a rule

- **WHEN** a domain event matches a retry, cooldown, setup-required, or health rule
- **THEN** Forge applies/projects the deterministic outcome without waking a model

#### Scenario: Self-event cannot recurse

- **WHEN** an agent-generated action produces an event in the same causation chain
- **THEN** configured reaction-depth and self-event rules prevent recursive wake beyond the bound

#### Scenario: Unchanged incident is deduplicated

- **WHEN** the same material incident state repeats during cooldown or an active lease
- **THEN** Forge creates no duplicate Attention item or agent analysis job

### Requirement: Initial Attention Categories

Attention SHALL remain a deterministic derived DTO/projection rather than mutable Task, Chat, or Agent truth. It SHALL support human input required, agent setup required, chat turn retrying/failed, validation failed, run stalled, retry exhausted, review ready, review risk, runtime offline, budget threshold, handoff failed, and commitment overdue as typed categories with bounded severity, source event, recommended action, and lifecycle.

#### Scenario: Actionable condition creates attention

- **WHEN** a qualifying event/current state requires a person or bounded agent response
- **THEN** Forge creates or updates one typed Attention item linked to the source entity/event

#### Scenario: Condition resolves

- **WHEN** authoritative state no longer satisfies the Attention condition
- **THEN** the projection resolves the item idempotently without mutating underlying Task, Chat, commitment, or binding truth

#### Scenario: Rebuild projection

- **WHEN** Attention materialization is discarded and rebuilt from events/current state
- **THEN** the same active identities, categories, severities, and recommended actions are produced
