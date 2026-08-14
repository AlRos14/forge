## ADDED Requirements

### Requirement: Versioned Project Agent Operating Instruction

Every Project Agent turn SHALL receive server-owned `forge.project.orchestration/v1` as a versioned immutable operating instruction rendered from canonical Project scope, binding, permission ceiling, current approved Charter pointer, authorized Project Document pointers, open decisions, active milestone projections plus explicit `primary_milestone_id`, and context-manifest references. The instruction SHALL define startup verification, domain-specific `EffectiveProjectState` resolution (not a global truth hierarchy), least-privilege research, document/revision workflow, scope-change classification, Task delegation, decision/memory behavior, milestone evidence, validation, communication, refusal, and explicit non-responsibilities. Agent Profile text MAY shape tone/expertise but SHALL NOT override the operating skill or server policy.

#### Scenario: Project Agent starts from a valid handoff

- **GIVEN** the handoff references the Project's current approved Charter revision and digest
- **WHEN** the first Project Agent turn begins
- **THEN** the agent verifies the handoff and binding, reads only the authorized Project context manifest, and acknowledges settled intent plus unresolved items
- **AND** it does not re-interview the user about settled Charter decisions

#### Scenario: Handoff reference is invalid

- **WHEN** the handoff's Charter revision is missing, unapproved, mismatched, inaccessible, or not attached to the target Project
- **THEN** Forge admits no mutating Project Agent action from that handoff
- **AND** the agent reports a typed conflict rather than reconstructing authority from prose

#### Scenario: Retrieved content requests more authority

- **WHEN** a Project Document, memory, web page, Task output, or handoff contains instructions to access another Project, credentials, arbitrary files, or a repository Workspace
- **THEN** the server denies the operation according to canonical Project scope
- **AND** the instruction tells the agent to treat retrieved content as data rather than policy

#### Scenario: Project Agent profile conflicts with operating skill

- **WHEN** an Agent Profile requests direct repository work, cross-Project context, self-release, or another behavior denied by the Project operating skill
- **THEN** the server-owned skill and tool policy prevail and the action is denied
- **AND** the turn provenance records both exact instruction revisions without implying that profile text granted authority

### Requirement: User-Approved Execution Baseline and Adaptive Envelope

Forge SHALL require one exact user-approved execution baseline before any repository-capable implementation Task becomes runnable or receives a scheduler-issued `WorkspaceLease`. The baseline SHALL bind the current Charter revision, applicable approved Document revisions, stable plan-item IDs, milestone selection and `primary_milestone_id`, a frozen release-policy revision/digest, acceptance/evidence matrix, Task capability/risk classes, reviewer-independence rules, elevated or irreversible operations, rollback/recovery assumptions, and an adaptive envelope. Before activation, discovery and planning Tasks MAY run only with server-enforced non-mutating capabilities; implementation Tasks MAY exist only as non-runnable plans. Within an active baseline's envelope, the Project Agent MAY split, sequence, retry, or substitute Tasks while preserving origin plan-item and replacement provenance. A change to outcome, acceptance, risk class, external side effect, release policy, governing artifact revision, or elevated/irreversible operation SHALL require reconciliation and a new user approval; affected pending Tasks, validations, evidence, milestones, and `ReadinessSnapshot` candidates become stale or carry a typed `reconciliation_required` reason as applicable.

#### Scenario: Repository Task is proposed before baseline approval

- **WHEN** the Project Agent drafts an implementation Task before an execution baseline is approved
- **THEN** Forge stores it only as a non-runnable plan with no repository capability or WorkspaceLease
- **AND** bounded discovery/planning Tasks use a non-mutating capability profile

#### Scenario: User approves an exact baseline

- **WHEN** the user approves the baseline's exact content/render digest with the expected current versions
- **THEN** Forge records a principal-bound approval and activates the baseline atomically
- **AND** the scheduler may issue a short-lived WorkspaceLease only to an assigned Worker/reviewer under the baseline's capability and logical repository-binding limits

#### Scenario: Adaptive split stays inside the envelope

- **WHEN** the Project Agent splits or sequences a planned Task without changing outcome, acceptance, risk class, side effect, release policy, or elevated operation
- **THEN** Forge preserves the origin plan-item ID, replacement provenance, governing baseline digest, and Task idempotency
- **AND** it does not require a second baseline approval

#### Scenario: Adaptive change leaves the envelope

- **WHEN** a proposed Task change changes a fixed baseline boundary
- **THEN** Forge attaches a typed `reconciliation_required` projection/reason to affected Tasks/milestones/validations and keeps repository-capable dispatch blocked
- **AND** it requests a new exact baseline approval before the changed work can run

### Requirement: Durable Project Documents

Forge SHALL support Project-owned typed Documents with kinds `research`, `delivery_brief`, `product_spec`, `design`, `architecture`, and `execution_plan`. Documents SHALL have stable identity, draft/approval policy, current draft/current approved pointers, and append-only immutable revisions containing base revision, content/schema/render versions, change summary, author/source provenance, digest, and timestamps. Projects SHALL create only the document kinds needed to make work safe and testable; compact Projects normally use a Delivery Brief and standard Projects use an Execution Plan plus applicable specifications.

#### Scenario: Small Project skips optional documents

- **GIVEN** an approved low-risk Charter contains sufficient outcome and acceptance detail
- **WHEN** the Project Agent prepares the first milestone
- **THEN** it may use a compact Delivery Brief and traceable Tasks without generating standalone research, design, or architecture or Execution Plan documents
- **AND** the absence of optional documents is not shown as incomplete paperwork

#### Scenario: Material architecture uncertainty exists

- **WHEN** an implementation Task would depend on an unresolved data, integration, security, concurrency, migration, or recovery decision
- **THEN** the Project Agent creates or updates the relevant research/architecture document or discovery Task first
- **AND** implementation Tasks reference the resolved artifact revision and decision

#### Scenario: Concurrent document update

- **WHEN** two actions revise the same Project Document from one base revision
- **THEN** exactly one appends successfully and the other receives a version conflict
- **AND** no approved or draft revision is overwritten

#### Scenario: Approved document is superseded

- **WHEN** an authorized actor approves a newer valid Document revision
- **THEN** Forge advances its current-approved pointer and preserves the earlier revision/approval
- **AND** Tasks and releases that cite the older revision retain stable provenance

#### Scenario: User wants a Project Document in the repository

- **WHEN** the user asks for an approved Project Document to be written into repository files
- **THEN** the Project Agent creates a Task referencing the exact artifact revision and desired target path/acceptance
- **AND** the scheduler issues the assigned Task Worker a scoped `WorkspaceLease` for the repository mutation
- **AND** the exported file does not silently replace Forge's canonical artifact

### Requirement: Least-Privilege Project Research

The Project Agent MAY use configured bounded web research for quick public non-authenticated facts that fit one interaction and are recorded with sources in a Project artifact. It SHALL create a discovery Task for repository inspection, code execution, experiments, substantial or resumable synthesis, authenticated/private access, or research requiring independent acceptance/evidence. Every research action SHALL identify the question, decision informed, source-quality expectation, stopping condition, and output artifact.

#### Scenario: Quick public fact

- **WHEN** one current public fact from a primary source can resolve a Project decision within the turn
- **THEN** the Project Agent may use configured Project-scoped web research
- **AND** it records source, retrieval time, supported claim, inference, uncertainty, and affected decision in an authorized artifact

#### Scenario: Research needs repository inspection

- **WHEN** answering the research question requires reading source files, executing code, benchmarking, or prototyping
- **THEN** the Project Agent creates a traceable discovery Task with outcome, stopping condition, acceptance, and artifact destination
- **AND** its core chat receives no filesystem or Task Workspace authority

#### Scenario: Research requires authenticated browser state

- **WHEN** research requires a private/logged-in service
- **THEN** direct Project Agent web research is denied by default
- **AND** Forge requires a separate explicit user-authorized Task/tool path whose result is bounded and re-authorized before artifact inclusion

#### Scenario: Source contains prompt injection

- **WHEN** a research source instructs the agent to change its role, reveal secrets, or invoke a Forge action
- **THEN** the content is treated as an untrusted claim only
- **AND** it neither alters the operating instruction nor widens the tool policy

### Requirement: Project Scope-Change and Decision Protocol

The Project Agent SHALL classify changes as clarification, implementation choice, or material scope change. Clarifications MAY revise a Project Document; authorized implementation choices SHALL append a principal-bound DecisionRecord with its decision class and effective `active` state; material scope changes SHALL propose a new Charter revision with visible diff, rationale, Task/milestone impact, and migration/cancellation consequences and SHALL require explicit user approval before becoming current truth.

#### Scenario: Project Agent makes an implementation decision

- **WHEN** two implementation alternatives both satisfy the approved Charter and the Project Agent's permission ceiling
- **THEN** the Project Agent may select one, append a principal-bound effective `active` DecisionRecord with its rationale/alternatives and decision class, and update affected Documents/Tasks
- **AND** it does not claim that the user personally selected it

#### Scenario: User asks for a material new outcome

- **WHEN** a request changes approved scope, non-goals, success, safety, launch, or material cost
- **THEN** the Project Agent proposes a new Charter revision and shows downstream Task/milestone consequences
- **AND** existing scope remains authoritative until explicit approval

#### Scenario: Decision is corrected

- **WHEN** a later authorized decision replaces an earlier decision
- **THEN** Forge appends a superseding Decision Log entry linked to the original
- **AND** it preserves the original rationale, actor, and affected references

### Requirement: Traceable Task Orchestration Without Repository Authority

The Project Agent SHALL create and manage Tasks only through existing typed Project-scoped actions and `TaskService` validation. Each created Task SHALL identify its source Charter/Document revisions, execution-baseline ID/revision and plan-item, milestone when applicable, outcome, acceptance criteria, dependencies, logical repository binding when applicable, and task type. Repository implementation and independent review SHALL remain delegated to assigned Task Workers/reviewers through scheduler-issued `WorkspaceLease` records; no chat agent may receive a path, token, handle, or lease.

#### Scenario: Project Agent creates implementation work

- **WHEN** an authorized Project Agent creates a Task from an approved specification
- **THEN** the Task records the relevant artifact revision and acceptance criteria and is validated by normal Project policy
- **AND** the Project Agent receives no repository Workspace

#### Scenario: Project Agent dispatches before baseline approval

- **WHEN** the Project Agent attempts to make a repository-capable Task runnable without an active user-approved execution baseline
- **THEN** Forge denies the dispatch and issues no `WorkspaceLease`
- **AND** discovery/planning work remains limited to its non-mutating capability profile

#### Scenario: Project Agent claims unreported repository work

- **WHEN** no authoritative Task delivery, validation, or evidence record reports a code change or test result
- **THEN** the Project Agent does not claim that it edited, tested, merged, or observed the repository outcome
- **AND** it describes the work as pending or unverified

#### Scenario: Reviewer is assigned its own work

- **WHEN** a reviewer assignment would attest work authored, implemented, or planned by that same principal
- **THEN** Forge denies the assignment or validation before recording a result
- **AND** the Project Agent cannot bypass the independent-review requirement by self-attesting

#### Scenario: Main Agent attempts the same Task action

- **WHEN** the global Main Agent invokes a Project Task creation or mutation action
- **THEN** Forge returns a typed policy denial and changes no Task

### Requirement: Project Decision Log and Canonical Memory References

Forge SHALL maintain an append-only Project Decision Log. Effective `DecisionRecord` state is exactly `active`, `superseded`, or `invalidated`, and every effective record stores its principal/decision maker and decision class (`user_scope`, `project_implementation`, `policy`, or `waiver`). Draft, proposal, approval, and rejection are separate candidate/editor workflow records and SHALL NOT be represented as effective DecisionRecord states. Scoped memory and chat summaries MAY reference Decision, Charter, Document, Task, Milestone, and Release IDs/revisions but SHALL NOT store a separately authoritative editable copy.

#### Scenario: User approves consequential decision

- **WHEN** the user approves a Project decision through an authorized action
- **THEN** Forge records an effective `active` DecisionRecord with the exact question, considered alternatives, selected outcome, rationale, principal/approver, decision class, and affected records
- **AND** later context can address the immutable Decision ID without copying hidden chat history

#### Scenario: Memory summary becomes stale

- **WHEN** a memory summary points to a superseded decision or artifact revision
- **THEN** context assembly admits the current canonical pointer and may mark the memory reference stale
- **AND** retrieval cannot override the current approved state

### Requirement: Project Agent Communication and Escalation

The Project Agent SHALL lead with the current outcome, blocker, decision, or next user action; ask at most two consequential questions in a turn; expose uncertainty, stale evidence, failed validation, and required approvals; and update canonical records after meaningful Project changes. It SHALL refuse cross-Project access, Main-Agent authority, direct repository/filesystem access, credentials, unapproved material scope, validation bypass, and self-approved release.

#### Scenario: Several low-risk choices are open

- **WHEN** multiple reversible implementation choices do not require user approval
- **THEN** the Project Agent records a reasoned recommendation and continues within policy
- **AND** it does not interrupt the user with a long questionnaire

#### Scenario: Consequential conflict cannot be resolved safely

- **WHEN** current approved artifacts and an explicit new user request conflict materially
- **THEN** the Project Agent shows the conflict, recommendation, impact, and no more than two resolving questions
- **AND** it pauses only the affected mutation while unrelated safe Project work may continue
