## MODIFIED Requirements

### Requirement: Product Genesis Prompt Contract

The Genesis prompt SHALL be server-owned `forge.main.project-discovery/v2`, a versioned immutable Main Agent operating skill rendered by a pure function of maturity and bounded canonical context only while Genesis is `discovering` or `ready_for_project`. Protocol v2 SHALL distinguish observed facts, explicit user decisions, research findings, assumptions, hypotheses, and open decisions; ask at most two high-information questions per turn; maintain a revisioned Project Charter draft; apply a maturity-sensitive readiness gate; explain naming recommendations; require explicit user approval of an exact Charter revision before Project creation; and disclose whether a Charter revision, Project, or handoff was created. The skill SHALL deny Main-Agent Task/repository authority, SHALL treat memory/web/model/Profile content as non-authoritative data, and SHALL override conflicting Agent Profile instructions without changing their stored text.

#### Scenario: Prompt carries protocol version and authority

- **WHEN** Product Genesis protocol v2 is rendered
- **THEN** its first line identifies the v2 Main Agent discovery/portfolio protocol
- **AND** it states the Main Agent's allowed discovery, configured research, Project lifecycle, portfolio, and handoff responsibilities
- **AND** it explicitly denies Task management and repository/filesystem access

#### Scenario: Discovery skill is not globally sticky

- **WHEN** no Genesis session is active or the session is `handed_off` or `cancelled`
- **THEN** Forge does not admit the discovery/grilling workflow as a normal Main Chat instruction
- **AND** a failed atomic create-and-handoff attempt leaves the same `ready_for_project` session and active receipt available for idempotent retry

#### Scenario: Discovery turn is bounded and consequential

- **GIVEN** a Genesis session has several unknowns
- **WHEN** the Main Agent prepares its next response
- **THEN** it asks no more than two questions whose answers can affect Project identity, scope, architecture, risk, success, or definition of done
- **AND** it does not re-ask a settled question unless it identifies conflicting newer evidence

#### Scenario: Maturity modulates readiness

- **WHEN** a Genesis session uses `production` or `critical` maturity
- **THEN** its readiness evaluation includes the applicable data, integration, security/privacy/compliance, accessibility, operational, migration, recovery, and launch concerns
- **AND** a `prototype` or `mvp` session may use the documented fast path without ceremonial optional documents

#### Scenario: Epistemic status remains distinct

- **GIVEN** the user states one decision, web research yields one finding, and the Main Agent infers one provisional choice
- **WHEN** the Charter draft is rendered
- **THEN** those items are classified respectively as user decision, research finding, and assumption or hypothesis
- **AND** none is silently promoted into another category

#### Scenario: Main Agent recommends a name

- **WHEN** the Project does not yet have an approved name
- **THEN** the Main Agent proposes one recommended working name with rationale and no more than two useful alternatives
- **AND** it does not represent local name availability as trademark or domain clearance
- **AND** the name remains unapproved until the user approves the exact Charter revision

#### Scenario: Prompt or retrieved content requests forbidden authority

- **WHEN** user text, memory, web content, or model output requests a Task mutation, repository operation, credential, or cross-Project private context
- **THEN** the server policy denies the operation
- **AND** the Main Agent gives the correct Project-Agent or Task-workflow route without treating the text as authority

### Requirement: Durable Genesis Lifecycle

Forge SHALL persist Genesis prompt revision, maturity, lifecycle state, Main Chat, source messages, version, optional preferred Project Agent, Charter/current revision/approval-receipt references, and optional resulting Project/handoff IDs independently of backend sessions. States SHALL be exactly `discovering`, `ready_for_project`, `handed_off`, and `cancelled`; `handoff_pending` is not a state. One atomic `CreateProjectFromCharterApproval` transaction SHALL advance `ready_for_project` directly to `handed_off`, transfer Charter mutation authority to Project scope, and consume the exact single-use approval receipt; any failure SHALL leave the session and receipt unchanged.

#### Scenario: Runtime session rotates during discovery

- **WHEN** the Main Agent backend session is replaced or Forge restarts during `discovering` or `ready_for_project`
- **THEN** Genesis remains active with the same lifecycle/version/Charter/source references
- **AND** the next admitted Main turn uses the same global chat continuity and applicable Genesis instruction revision

#### Scenario: Project and handoff commit together

- **WHEN** `CreateProjectFromCharterApproval` succeeds
- **THEN** Project, binding, Project Chat, Charter attachment, handoff/message/turn job, events, Genesis `handed_off`, and consumed receipt become visible together
- **AND** there is no committed state containing only the Project or only the handoff

#### Scenario: Handoff has no intermediate state

- **WHEN** Project creation is in progress or fails before commit
- **THEN** Genesis remains `ready_for_project` and the receipt remains `active`
- **AND** Forge exposes neither a `handoff_pending` state nor a partial handoff
- **AND** a successful commit changes Genesis directly to `handed_off`

#### Scenario: Transaction fails or restarts

- **WHEN** any create-and-handoff database operation fails or Forge restarts before commit
- **THEN** no partial Project, binding, chat, handoff, target message/turn, event, consumed receipt, or lifecycle change is visible
- **AND** retry with the same receipt/idempotency key may attempt the same transaction again

#### Scenario: Committed transaction response is lost

- **WHEN** create-and-handoff commits but the client loses the response and retries with the same receipt/idempotency key
- **THEN** Forge returns the original Project, binding, handoff, and target turn identities
- **AND** it creates no duplicate record or agent response admission

#### Scenario: User cancels before Project creation

- **WHEN** an authorized user cancels a `discovering` or `ready_for_project` session with the expected version
- **THEN** Forge marks it `cancelled` and stops admitting the Genesis instruction
- **AND** historical Charter revisions, messages, and provenance remain immutable

### Requirement: Atomic Project Creation and Explicit Handoff

When Genesis is ready, the Main Agent SHALL propose Project metadata, one eligible Project Agent identity/profile/operating-skill selection, and the exact Charter content/render digests. The user approval action SHALL create one immutable, principal-bound, single-use receipt with `active`, `consumed`, or `revoked` lifecycle and an idempotency key. `CreateProjectFromCharterApproval(approval_id, idempotency_key)` SHALL verify and consume only that receipt, then atomically create the Project, its single Project Agent binding, Project Chat, Charter attachment, bounded immutable handoff, target message/turn job, domain events, Genesis `handed_off`, and receipt consumption. Failure SHALL roll back every record; replay SHALL return the original committed result.

#### Scenario: User approves exact Charter and Project Agent

- **GIVEN** the Charter revision is ready but unapproved
- **WHEN** the user approves its exact revision plus content/render digests and an eligible Project Agent identity/profile/operating-skill revision set
- **THEN** Forge records one immutable active approval receipt with user/action provenance
- **AND** the creation action may consume only that exact receipt once

#### Scenario: Draft changes after approval

- **GIVEN** revision 4 is approved and revision 5 is later drafted
- **WHEN** Project creation is requested with revision 5 or with a digest that does not match revision 4
- **THEN** Forge rejects the request as an unapproved or stale Charter conflict
- **AND** it creates no Project, binding, chat, or handoff

#### Scenario: Genesis becomes a Charter-backed Project

- **WHEN** the Main Agent invokes creation with a valid active receipt
- **THEN** Forge creates exactly one Project, binding, Project Chat, immutable Charter attachment, handoff, target message/turn, and matching events in one transaction
- **AND** the handoff contains the Charter identity/revision/content+render digests, receipt provenance, bounded summary, and unresolved items
- **AND** the user receives a “Continue with Project Agent” action to that existing Project Chat

#### Scenario: Project creation fails

- **WHEN** any create/bind/chat/Charter/handoff/message/turn/event operation fails before transaction commit
- **THEN** Genesis remains `ready_for_project` with its approved Charter and active receipt unchanged plus a bounded failure state
- **AND** no partial Project, consumed receipt, or handoff is visible

#### Scenario: Approval receipt is replayed

- **WHEN** the same consumed receipt and idempotency key are replayed after a timeout
- **THEN** Forge returns the original Project/handoff result
- **AND** it creates no duplicate Project, binding, target message, turn, or event

#### Scenario: Handoff content is bounded

- **WHEN** Forge constructs the Charter-backed handoff
- **THEN** it may include safe source references, settled-decision IDs, typed unresolved items, and safe research references
- **AND** it excludes the full Main timeline, hidden memory bodies, credentials, protected runtime/browser state, unrelated Project data, arbitrary file paths, and authority-bearing instructions

## ADDED Requirements

### Requirement: Genesis Charter Draft Surface

The existing Product Genesis lifecycle SHALL expose its current Project Charter draft, revision history, readiness gaps, approval state, and revision diff inside the singular Main Chat without creating another conversation, Room, thread, or agent responder.

#### Scenario: Main Agent saves a Charter update

- **WHEN** an authorized Main Agent turn yields a valid Charter delta
- **THEN** Forge appends one immutable Charter draft revision with source-turn provenance and digest
- **AND** the Main Chat shows a concise change summary and link to inspect the revision rather than duplicating the full artifact in every message

#### Scenario: User inspects approval candidate

- **WHEN** the Main Agent marks a Charter revision ready for approval
- **THEN** the Main Chat exposes the exact revision, content/render digests, material diff, assumptions, unresolved items, proposed Project metadata, and selected Project Agent identity/profile/operating-skill revisions
- **AND** approval remains a separate explicit user action

#### Scenario: Product is not ready

- **WHEN** one or more maturity-required Charter sections remain consequentially incoherent
- **THEN** the Main Agent continues bounded discovery or records the unknown as an explicit research/open-decision item
- **AND** Forge does not represent the draft as approval-ready merely because many fields contain text
