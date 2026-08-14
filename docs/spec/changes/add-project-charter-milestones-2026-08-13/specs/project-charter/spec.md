## ADDED Requirements

### Requirement: Revisioned Project Charter

Forge SHALL persist a stable Project Charter and append-only immutable Charter revisions. A Charter MAY be owned initially by one active Product Genesis session with no Project, SHALL attach to exactly one Project during atomic creation, and SHALL never be re-parented. Each revision SHALL include typed content, the exact rendered representation shown for approval, schema/render version, base revision, monotonic revision number, change summary, author/source provenance, canonical content digest, rendered-view digest, and timestamp.

#### Scenario: Save a draft revision

- **GIVEN** Charter revision 2 is current
- **WHEN** an authorized actor saves a valid delta using revision 2 as its base
- **THEN** Forge appends revision 3 and preserves revision 2 byte-for-byte
- **AND** the response identifies the change summary, content/render digests, and source provenance

#### Scenario: Concurrent draft writers

- **WHEN** two authorized actions save against the same current Charter version
- **THEN** exactly one appends the next revision
- **AND** the other receives a version conflict with no hidden merge or overwritten content

#### Scenario: Attach Genesis Charter during Project creation

- **WHEN** `CreateProjectFromCharterApproval` consumes an active approval receipt
- **THEN** Forge attaches that receipt's exact Charter revision and creates the Project/binding/chat/handoff in the same transaction
- **AND** the Charter cannot later be attached to another Project or Genesis session

#### Scenario: Project creation races a newer Charter approval

- **GIVEN** one action is consuming the active approval receipt while another attempts to approve a newer Genesis revision
- **WHEN** the actions contend on the Charter/Genesis/receipt version
- **THEN** exactly one ordering commits and a newer approval revokes any still-active older creation receipt
- **AND** Project creation either attaches the receipt's still-current exact revision or fails stale without creating partial state
- **AND** no later Main-scope approval can mutate the Charter after Project attachment

#### Scenario: Approved Project name or slug conflicts at creation

- **WHEN** the exact approved Charter metadata fails current Project name/slug uniqueness or format validation
- **THEN** Forge creates no partial Project and returns a typed conflict with safe corrective guidance
- **AND** neither Forge nor the Main Agent silently substitutes a different user-facing name or approved identity

#### Scenario: Main Agent bypasses Genesis Charter approval

- **WHEN** the Main Agent invokes any Project-creation action without an active receipt bound to the exact current Charter content/render digests and selected Project Agent identity/profile/operating-skill revisions
- **THEN** Forge denies the action and creates no Project
- **AND** a separate authorized human/API creation path may still create an explicit `charter_setup_required` Project under normal policy

#### Scenario: Content and rendered-view digests are deterministic

- **WHEN** canonical Charter content and its schema-versioned rendered view are produced more than once
- **THEN** Forge computes stable independent content and rendered-view digests regardless of JSON object field order
- **AND** approval binds both digests so the reviewed display cannot differ from the canonical payload

### Requirement: Charter Content and Readiness

A Project Charter SHALL declare `project_mode` as `compact` or `standard` and represent identity; problem and people; core experience; initial scope and explicit non-goals; definition of success; material constraints and risks; a knowledge ledger; and source/change provenance. Readiness SHALL validate meaningful mode/maturity-specific content and unresolved-item classification rather than only field presence.

#### Scenario: Small Project uses the fast path

- **GIVEN** a low-risk Project uses `compact` mode and has a coherent name, outcome, success check, explicit non-goals, material constraints, and visible unresolved queue
- **WHEN** readiness is evaluated
- **THEN** Forge may mark the revision ready for one-action approval without requiring standalone research, product, design, architecture, or Execution Plan documents

#### Scenario: Critical Project omits safety concerns

- **GIVEN** a `critical` maturity Charter has no resolved or queued data-sensitivity, security/privacy/compliance, failure/recovery, and operational concerns
- **WHEN** readiness is evaluated
- **THEN** Forge returns typed readiness gaps
- **AND** the revision cannot be approved as ready until each gap is resolved, explicitly inapplicable, or queued with visible risk

#### Scenario: Unknown is represented honestly

- **WHEN** the user cannot yet answer a consequential question
- **THEN** the Charter records a reversible assumption, hypothesis, open decision, or research item with impact
- **AND** it does not fabricate an observed fact or user decision

### Requirement: Explicit Charter Approval and Supersession

Forge SHALL record user approval as an immutable, principal-bound receipt tied to an exact Charter revision, content/render digests, and—for pre-Project Genesis approval—the selected Project Agent identity/profile/operating-skill/policy revisions. The receipt SHALL include the approving principal, authorization basis, expected Charter version, explicit UI/command event, timestamp, and idempotency key. A receipt SHALL have `active`, `consumed`, or `revoked` lifecycle and be single-use for Project creation. A Project SHALL have at most one current approved Charter pointer. Approving a later revision SHALL advance that pointer with optimistic concurrency, revoke any still-active older creation receipt, and preserve earlier approval/revision history. Silence, conversation continuation, agent output, and Task progress SHALL NOT count as approval.

#### Scenario: User approves an exact revision

- **WHEN** an authorized user approves a ready Charter revision with matching expected version plus content/render digests
- **THEN** Forge appends one approval receipt and advances the current-approved pointer atomically
- **AND** retries with the same deduplication key return the same approval

#### Scenario: Approval races a newer draft

- **WHEN** a user submits approval using a stale Charter version or mismatched digest
- **THEN** Forge returns a conflict and records no approval
- **AND** the UI shows the newer diff before inviting another decision

#### Scenario: Project Agent proposes material scope change

- **GIVEN** the Project has an approved Charter
- **WHEN** the Project Agent proposes a change to target user, core loop, in-scope outcome, explicit non-goal, success measure, material constraint, safety posture, launch commitment, or expected cost
- **THEN** Forge appends a typed `CharterAmendment` in `draft` or `proposed` state with amendment ID, base/candidate revisions and content/render digests, rationale, material diff, requested principal, expected current Charter version, and affected Decisions, Documents, Tasks, execution baselines, validations, evidence, and Milestones
- **AND** the prior revision remains current until an authorized user explicitly approves the proposal

#### Scenario: User approves Charter amendment

- **WHEN** an authorized user approves an amendment with the expected current Charter revision and candidate content/render digests
- **THEN** Forge advances the current Charter pointer atomically and records the immutable principal-bound approval
- **AND** affected Decisions, Documents, Tasks, execution baselines, validations, and Milestones receive a typed `reconciliation_required` projection/reason until each is explicitly retained, revised/replaced, cancelled, invalidated, or superseded with actor and reason; this does not add a lifecycle state to an effective `DecisionRecord`

#### Scenario: Amendment approval races current Charter change

- **WHEN** the current approved Charter pointer differs from the amendment's expected base at approval time
- **THEN** Forge returns a version conflict and leaves the amendment unapproved
- **AND** no downstream record is silently reconciled against the wrong baseline

#### Scenario: Main Agent attempts Charter revision after attachment

- **WHEN** a Main Agent/Genesis action tries to draft or approve a Charter after it is attached to a Project
- **THEN** Forge returns an ownership-transfer policy denial
- **AND** the user is directed to the singular Project Agent Chat for any proposed Project revision

#### Scenario: Implementation clarification stays below Charter level

- **WHEN** the Project Agent makes a reversible implementation choice that does not alter approved Project identity, scope, risk, cost, or acceptance
- **THEN** it records the choice in the Decision Log and relevant Project Document
- **AND** it does not create a misleading Charter supersession

### Requirement: Domain-Specific Effective Project State

Forge SHALL expose a typed `EffectiveProjectState` projection that resolves claims by authority domain rather than applying a global latest-record or universal truth hierarchy. The projection SHALL identify the current approved Charter for identity/scope, the active approved execution baseline and its exact Document revisions for execution intent, active compatible Decisions, latest server-accepted Task events for work state, principal-bound validations for check truth, immutable release manifests for released claims, reconciliation-required records, canonical conflicts, active milestones and explicit `primary_milestone_id`, readiness, release revisions, and the source event watermark. Human-readable status, chat, memory, and dashboard projections SHALL remain non-authoritative.

#### Scenario: Domain authorities disagree

- **WHEN** a current Document or Decision conflicts with the approved Charter or active baseline in its governing domain
- **THEN** Forge records a `canonical_conflict` naming the records, revisions, digests, authority domain, governing record, and affected Tasks/milestones/checks
- **AND** it blocks only the affected execution/readiness paths and attaches a typed `reconciliation_required` projection/reason to affected records
- **AND** it does not resolve the conflict by recency, chat prose, memory, or a global latest-record rule

#### Scenario: Effective state is rebuilt

- **WHEN** a Project Overview or Project Agent turn requests current state
- **THEN** Forge rebuilds `EffectiveProjectState` from its named authority sources and event watermark
- **AND** a stale or failed projection is shown as stale/error without changing canonical records or release truth

### Requirement: Charter Context and Memory Authority

Forge SHALL make exact authorized Charter revisions first-class context-manifest sources. Project Agent context SHALL receive the current approved revision plus only relevant authorized proposals/open items. Main Agent context SHALL receive only Genesis-owned drafts and bounded portfolio projections. Semantic memory and LCM summaries MAY reference Charter IDs/revisions/content+render digests but SHALL NOT become separately editable copies of Charter truth.

#### Scenario: Project Agent turn admits Charter context

- **WHEN** a Project Agent turn requires Project identity or scope
- **THEN** its context manifest records the exact current approved Charter revision, content/render digests, inclusion reason, and token disposition
- **AND** the turn can inspect provenance without receiving hidden Main Chat history

#### Scenario: Memory conflicts with current Charter

- **GIVEN** semantic memory recalls an older scope statement
- **WHEN** the current approved Charter contains a newer superseding decision
- **THEN** the Project Agent follows the current Charter and identifies the memory as stale retrieval context
- **AND** no memory promotion overwrites or widens the approved artifact

#### Scenario: Cross-Project Charter reference

- **WHEN** a Project Agent submits another Project's Charter or revision ID
- **THEN** authorization rejects the reference before content, counts, snippets, or digest metadata are returned

### Requirement: Existing Project Charter Adoption

Projects that predate this change SHALL preserve all existing data and remain usable while exposing explicit `legacy_unverified` and `charter_setup_required` status. Forge MAY generate an unapproved `legacy_unverified` adoption draft from authorized current state with unknowns and source provenance, and the Project Agent MAY refine it, but Forge SHALL NOT fabricate user decisions or approval. The legacy adoption draft remains unapproved until the user explicitly approves its exact revision; no migration step may silently promote it. `charter_setup_required` SHALL block release only; Project Chat, Tasks, evidence, and Document maintenance remain usable. Milestone release SHALL require an approved Charter unless an explicit migration policy says otherwise.

#### Scenario: Existing Project opens after migration

- **WHEN** a Project has no safely inferable approved Charter
- **THEN** its Tasks, Project Chat, evidence capture, and Document maintenance remain usable and the Project Overview shows `charter_setup_required`
- **AND** Forge does not synthesize an approved Charter from old chat, Task, or memory text

#### Scenario: User approves adoption Charter

- **WHEN** the Project Agent proposes a Charter from authorized existing Project facts and the user approves its exact revision
- **THEN** Forge establishes it as the Project's current approved Charter
- **AND** source records are preserved as provenance rather than silently treated as prior approval
