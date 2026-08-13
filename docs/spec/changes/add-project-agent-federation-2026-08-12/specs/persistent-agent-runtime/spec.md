## ADDED Requirements

### Requirement: Capability-Aware Forge-Hosted Session Backend

Forge SHALL execute native embedded-agent turns through a capability-aware `AgentSessionBackend` whose reference implementation composes the exact pinned Agent Runtime revision inside a Forge-owned host. The host SHALL bind server-issued identity/scope, protected persistence, provider/model configuration, context sources, typed tools, event handling, and scope-derived workspace. Committed dependency manifests SHALL NOT rely on sibling filesystem paths.

#### Scenario: Start a native Main or Project Chat session

- **WHEN** an eligible active binding admits a native Main or Project Agent Chat turn
- **THEN** Forge composes the session with protected host adapters and the exact chat-scoped policy
- **AND** the stored capability snapshot describes only controls that backend actually supports

#### Scenario: Task scope receives its admitted Workspace

- **WHEN** a native session starts for an eligible Task Worker or reviewer assignment
- **THEN** the host derives filesystem/tools from that Task's current role and workflow
- **AND** the grant is absent from every Main and Project Agent Chat session for the same identity

#### Scenario: Unsupported capability is rejected

- **WHEN** a backend does not support steering, exact checkpoint recovery, or another optional capability
- **THEN** Forge does not present the control as available
- **AND** a direct attempt returns a structured capability error without changing session state

#### Scenario: Local sibling override is not committed

- **WHEN** developers validate Forge against `../agent-runtime`
- **THEN** they use a git-ignored Cargo patch override
- **AND** the committed manifest still resolves the exact approved revision

### Requirement: Identity-Scope LCM Continuity

Forge SHALL bind one logical LCM timeline to an authorized `(AgentIdentity, canonical_scope_type, canonical_scope_id)` independently of replaceable runtime-session identity. Main and Project conversation turns SHALL use their owning Agent Chat as canonical scope, and native Task work SHALL use the Task. Forge's SQLite adapter SHALL implement Agent Runtime LCM authorization, immutable append, transactional compare-and-swap DAG mutation, deterministic projection, and bounded expansion.

#### Scenario: Restart continuity

- **WHEN** Forge restarts after an identity accumulated chat or Task history and LCM summaries
- **THEN** the resumed/replacement session binds the same authorized timeline
- **AND** the next turn uses the active lossless projection and recent raw suffix without requiring the old process

#### Scenario: Binding or backend rotation preserves timeline

- **WHEN** Forge changes profile/provider/backend session while the same identity remains bound to the same chat
- **THEN** it creates a replacement session under the same timeline
- **AND** chat history, commitments, and LCM frontier remain unchanged

#### Scenario: Binding replacement preserves historical attribution

- **WHEN** a different identity replaces the responder binding for an existing chat
- **THEN** future turns bind that identity's chat-scoped timeline
- **AND** Forge does not merge the old identity's private episodic timeline into the replacement's context

#### Scenario: Scopes do not merge

- **WHEN** one identity has Main Chat, several Project Chat, and Task timelines
- **THEN** each timeline admits only records authorized for that canonical scope
- **AND** shared identity, profile, handoff references, or session rotation never merges histories

#### Scenario: Opaque pointer grants no authority

- **WHEN** a caller presents a valid timeline, node, cursor, or pointer without the host-issued authorized view
- **THEN** Forge returns no source or summary content

#### Scenario: Concurrent compaction is atomic

- **WHEN** two processes attempt a compatible LCM mutation from the same expected revision
- **THEN** exactly one mutation commits and the loser receives a revision conflict
- **AND** no partial node, edge, or supersession state is visible

#### Scenario: Secret-class history is not summarized

- **WHEN** an eligible compaction span contains secret-class source content
- **THEN** it is not sent to a summary model or stored in a normal LCM summary body
- **AND** it remains protected/raw or execution returns a structured cannot-fit outcome

#### Scenario: Summary restrictions preserve provenance

- **WHEN** LCM derives a node from sources with different sensitivity, trust, guard, or transformation revisions
- **THEN** the node carries the most-sensitive/least-trusted join and exact revision provenance
- **AND** the derived body passes the active content guard before commit

### Requirement: Single Context Planning Authority and Reproducible Manifest

For native turns, Forge SHALL deterministically authorize/select domain sources for the canonical Agent Chat or Task and persist stable fragment IDs, revisions, selection reasons, scope/policy revision, and sensitivity in a `ContextManifest`. Agent Runtime SHALL remain the sole authority for final ordering, token accounting, compaction, serialization, LCM projection, and cache identity. Forge SHALL derive final dispositions from the immutable `RunManifest` rather than independently re-plan it.

#### Scenario: Identical state produces identical fingerprint

- **WHEN** admitted sources, policies, LCM frontier, runtime/context revisions, model limits, and request are unchanged
- **THEN** context admission produces the same combined manifest fingerprint

#### Scenario: Handoff source revision changes fingerprint

- **WHEN** an admitted handoff, Decision, commitment, memory record, Task snapshot, policy, or LCM node changes
- **THEN** the next turn records a different fingerprint and identifies the changed revision

#### Scenario: Inspect provenance safely

- **WHEN** an authorized user inspects a turn's context provenance
- **THEN** Forge shows admitted source identities/revisions/reasons and final included/summarized/omitted dispositions
- **AND** it omits secrets, protected checkpoints, inaccessible memory bodies, and authority grants

### Requirement: Protected Runtime State Separation

Provider credentials, exact checkpoints, staged LCM responses, interaction secrets, and host authority grants SHALL use a protected store/API boundary. Ordinary profile, binding, chat, message, turn, session, memory, handoff, manifest, event, and log records SHALL contain only opaque references and bounded safe status.

#### Scenario: Runtime failure is redaction-safe

- **WHEN** provider connection, protected session persistence, checkpoint recovery, content guarding, LCM mutation, or turn failure persistence fails
- **THEN** user/audit diagnostics contain typed bounded reasons and opaque IDs only
- **AND** no protected body, credential, authority token, or provider secret is emitted
