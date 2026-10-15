# Independent orchestration architecture: Plan PR0 contract

Status: accepted migration target. PR13's V121–V134 destructive cutover and
admission reliability corrections are implemented in the current branch and
remain subject to PR review and merge.

This document records the accepted architecture and migration ledger. The
Plan PR0 baseline and pre-PR13 dependency audit below are historical snapshots;
the implementation status section records the current storage cutover.

## Scope statement

Plan PR0 changes documentation only:

* it defines the independent Actor, Agent, Harness, Role, Execution,
  ValidationRun, HarnessSession, WorkUnit, Artifact, Evidence, Gate, and
  collaboration model;
* it records the distinction between orchestration, cognition, and deterministic
  authority;
* it documents the migration order, compatibility rules, ADRs, and current
  dependency audit;
* it records the upstream relationship as attribution rather than a
  compatibility obligation.

Plan PR0 does not change:

* Rust code, crate dependencies, runtime composition, or process behavior;
* SQLite migrations, tables, readers, writers, or persisted data;
* REST, MCP, CLI, SSE, or web contracts;
* planning, review, workflow, scheduling, execution, or agent-host behavior;
* product branding, binary names, package names, config paths, or database
  locations;
* deployment, publication, or upstream integration.

## Invariants touched

The documentation establishes all INV-001 through INV-037 below. No runtime
invariant is claimed as enforced by this Plan PR. Later Plan PRs must name the subset
they enforce and add focused tests before changing the corresponding code.

## Plan PR4 implementation contract

Plan PR4 adds the generic `Artifact`, `Message`, `Handoff`, `Proposal`, and
`Decision` persistence and `/api/v1` surface. These records are authoritative
for data created through that generic surface. Existing verticals remain
authoritative for their own records until their assigned migration. There is
no implicit projection or dual-write between the two families.

In particular, creating `task_plan_revision`, `agent_chat_message`,
`agent_handoff`, or `project_decision` does not create a generic record; generic
writers do not update those legacy tables. Planning migrates in PR7, Review and
Validation in PR8, Task/gate contracts in PR9, Agent Host and Project OS in
PR11, and physical legacy cleanup in PR13. Other records listed in the legacy
dependency audit keep their named migration assignments.

PR4 Artifact provenance is represented by the normalized
`artifact_execution_producer` relation. PR4 supports an Execution producer
only; its ActorRef is derived through Execution and is not duplicated on
Artifact. PR8 may add `artifact_validation_run_producer` additively after
ValidationRun exists. PR4 creates no nullable FK to a future table and does not
invent a System Actor.

Message is communication; Handoff addresses work and has a controlled status
lifecycle but does not assign a Role or create RoleMembership. Proposal records
an immutable intent and policy evidence, not a command. Decision records one or
more Human/Agent deciders and an immutable outcome; it does not execute the
Proposal. Policy references and snapshots are evidence, not executable policy,
permissions, or authorization.

`domain_event` remains the durable event ledger. Each PR4 record mutation and
its domain event commit in one transaction; the in-process EventBus is only a
post-commit notification. Payloads exclude bodies, content, rationales,
filesystem paths, storage locators, free-form Artifact digests, free-form
Proposal actions, policy references, and authorization material. Artifact and
Proposal records retain those fields for authorized readers. Project teardown
removes PR4 rows through the guarded Project deletion path; the event ledger
remains historical evidence under its existing retention contract.

## Plan PR7 implementation status

V098 makes planning outputs generic immutable Artifacts and exact Execution
inputs. The unique `(execution_id, kind)` output relation makes retries return
the same Artifact, while `execution_artifact_input` pins a same-Task Artifact
and digest before a downstream Execution starts. Plan Artifact provenance is
derived from its real Human or Agent Plan Execution. V081 revision and approval
rows were no longer read or written by planning, workflow gates, REST plan
projection, or review evidence selection. V098 maps only rows whose source
Execution has a verifiable same-Task Human/Agent Actor; unattributable and
ambiguous rows retain an explicit audit status and no invented producer.
V125 preserves the exact V081 rows, drops their runtime tables and `task.plan`,
and keeps the V098 ledger without its source-row foreign key.
The workflow resolver also discards the retired checklist hook from previously
stored workflow definitions, and failed Plan Executions no longer create a
planner-specific retry exception; general workflow and execution recovery stay
with their owning migration plans.

## Plan PR8 implementation status

V099 adds Actor-free `validation_run` and producer-bound deterministic
`evidence`, plus `artifact_validation_run_producer`. A new ValidationRun
freezes Task, Workspace, commit, working-tree snapshot digest, check/command,
bounded environment summary, and an idempotency key. Terminal result, Evidence,
optional validation-report Artifact, and their domain events commit together.
Evidence SQL guards compare its same-Task producer and its recorded check,
configuration, Workspace, commit, snapshot, status, and exit code.

Cognitive review is now a real Human or Agent reviewer Execution with purpose
`review`. Completion requires its unique exact `review_report` Artifact;
structured `FORGE_RESULT` remains only the harness transport parsed into that
Artifact. New deterministic checks create ValidationRuns and Evidence and do
not create or update a Review row. Review output and Validation output remain
independent. TaskLifecycle and Gate policy own current transition admission;
retired workflow hooks and review rows cannot affect it.

Each workspace-bound Review Execution freezes one immutable
`review_execution_subject` row with its exact Task, Workspace, base commit,
head commit, and working-tree snapshot digest. Human Review freezes at start
and rechecks all five identities at submission. A local Agent Review freezes
after acquiring the workspace execution lock and its final WorkspaceLease
revalidation, directly before launch; remote Review freezes directly before
provider launch. Both Agent paths recheck the same identity before
materializing a ReviewReport. Automatic Validation Evidence is pinned only
when its ValidationRun matches the frozen Workspace, head, and snapshot digest.
ReviewReport repeats the complete frozen subject, and SQL guards require an
exact match.

Local read-only Review captures and restores the exact pre-review HEAD, Git
index, tracked diff and file permissions, and non-ignored untracked files under
the Workspace lock. Its isolated restore snapshot stays outside the worktree.
It verifies the restored snapshot digest before report materialization and
fails without a report if restoration cannot be proven. The durable v2
Workspace snapshot digest covers staged index-visible state, tracked
working-tree state, and untracked state; it does not hash raw Git index bytes.
The temporary restore snapshot may retain those exact bytes to restore local
state. Review and ValidationRun use this same semantic digest.

Human start with or without a Workspace and new Human ReviewReport creation
recheck current RoleMembership-first authority in their write transactions. An
exact persisted ReviewReport with a still-Running Execution is a recoverable
crash boundary: validate its historical producer, ActorRef, subject, content,
and digest, then complete and cascade from that Artifact without current
membership or live Workspace revalidation. The same rule applies to Agent
report recovery; it does not rerun review cognition. Conflicting retries fail
closed.

When a replacement TaskRole exists, its RoleMembership records alone decide
current Human reviewer authority; the singular TaskRoleAssignment row is only a
projection. A Human may start a Review Execution for each active membership,
even when the compatibility projection names another Human. The bounded
legacy singleton fallback applies only before a replacement TaskRole exists.
Human submission pins the exact requested Evidence and Artifacts, creates or
reuses the one ReviewReport output, and appends its Artifact event in one DB
transaction. A failed submission leaves no partial input bindings.

Workflow hook context now carries an Execution ID only when the transition
names that exact completed Review Execution as its cause. Manual transitions
and entry-barrier retries do not inherit a recent Execution as cause. Reviewer
dispatch never resumes a prior thread by role recency; request-changes starts a
fresh work-role Execution under the active workflow and does not infer a prior
role HarnessSession.

The generic `/gates/review/{approve,reject}` compatibility routes require a
matching Task version and exactly one running Human reviewer Execution for the
authenticated user; they submit a ReviewReport to that exact Execution. The
older `/review/{approve,reject}` URLs return 409 because their response shape
requires a legacy Review row. Neither route writes new legacy Review state.

V099 retains all legacy Review, V084 bundle, and Task compatibility storage.
It migrates only structured review output with same-Task real ActorRef,
reviewer/review producer Execution, and a complete reconstructible report with
no unpinned legacy Evidence references. Other rows receive an explicit audit
status. Legacy CI is audited but not backfilled when its persisted environment
and Workspace snapshot cannot be proven. V131 archives and drops the Review
and evidence-bundle tables, preserves detached V099 migration ledgers, and
retains no special Review runtime. The implementation details and the
pre-cutover transition/recovery audit are recorded in
`plan-pr8-review-validation-evidence.md`.

## Plan PR9 implementation status

PR9 is implemented on branch `feat/plan-pr9-gate-engine-task-lifecycle` from
the post-PR8 `main` base recorded in
`plan-pr9-gate-engine-task-lifecycle.md`. V100 adds aggregate
`task_lifecycle`, conservative migration audit, scoped Gate identity,
immutable policy revisions/evaluations, exact input refs, and transition
receipts. V101 adds immutable retry receipts over exact failure facts. V102
emits TaskRole change events and permits guarded Project teardown to remove
retry receipts; TaskRole policy edits must advance the role version fence.
V103 fences exhausted retry budgets, V104 rechecks exact mutable Gate inputs at
lifecycle and merge effects, and V105 permits merge-ready rework only from a
current GateEvaluation or verified retry receipt that has not been superseded
by a later lifecycle transition.

`TaskLifecycleService` owns aggregate progress, version fencing, causal
identity, idempotency, the one-way `task.status` projection, and durable
events. `GateEngine` evaluates bounded review, ValidationRun/Evidence,
Decision, WorkUnit, exact TaskMerge operation, and exact lifecycle transition
requirements. Durable fact events drive evaluation; EventBus is only a wake
hint. A Gate-caused transition stores the exact GateEvaluation, and replay
applies that persisted evaluation rather than recomputing against newer facts.
V106 fences Running Execution insertion/resumption against terminal Task
lifecycle in SQLite. V107 binds an `interactive` Execution's WorkspaceLease to
the TaskRole selected by Task type when that role exists, and checks active
Agent membership on lease issue and renewal. The old singleton fallback stays
available only when the matching canonical TaskRole does not exist.

An Execution-owned TaskLifecycle block is resolved only by admission of a
child whose exact parent is the current blocking Execution on the same open
WorkUnit. The current lifecycle reference and transition receipt, when
present, plus each present `error_annotation` and `blocked_json` must identify
that Execution. A sibling WorkUnit admission is rejected while another
WorkUnit owns the aggregate block. Block cleanup, lifecycle activation, child
Execution, ArtifactInputs, WorkspaceLease, receipts, and durable events commit
together. Parentless launch and generic lifecycle transitions cannot bypass
pending block evidence; Task cancellation remains an explicit Task-wide
resolution.

V115 repairs the narrowly identifiable historical case where provider Merged
on a different PR head was turned into automatic Task rework, but only when
that exact rework still owns the current lifecycle. It preserves the old retry,
event, and transition facts and appends a durable integrity repair plus a
blocking lifecycle transition. V116 snapshots terminal modern PR facts before
reusing the Task's mutable `pr_metadata` projection. Exact historical provider
callback replay resolves against the frozen admission and result event, so a
later PR cannot invalidate replay of an earlier terminal result.

V117 revisits only V115's missed wrong-head rework cases. It repairs when the
exact erroneous Blocked-to-Active transition still owns the current lifecycle
and no later lifecycle transition exists. Unrelated Decisions and retry events
without a lifecycle effect do not supersede that authority; a later real
lifecycle transition still does. The repair preserves existing retry and
lifecycle history and appends a versioned repair fact and blocking transition.

PR9 does not create HumanApproval, copy review or validation verdicts, or
convert legacy workflow definitions into Gate policies.

For merge readiness, ReviewReport and ValidationRun subjects must share the
same Workspace, commit, and snapshot. WorkUnit-backed merges bind to an exact
successful integration. A merge Gate can require only passing ValidationRuns
and approving Decisions. Reviewer TaskRole snapshots include the current role
version and exact membership observation; membership mutations advance the
role fence and append a durable fact event. Gate and its first policy revision
commit together. MergeService acquires the cross-process Task integration lock
before reading its candidate, verifies the caller supplied the current
satisfied evaluation, checks the Workspace commit, and retains the lock through
atomic merge admission. Durable insertion order identifies the latest
evaluation, so replay cannot promote an older receipt.

Retry budgets remain outside Gate authority. V101 receipts deduplicate exact
ReviewReport request-changes, failed ValidationRun, failed Execution, and merge
failure facts. Rework produces a durable Orchestrator event; exhaustion blocks
the aggregate lifecycle and Execution admission. An exact failure replay does
not consume another attempt, and a rework fact does not release a block owned
by an unrelated cause.

TaskLifecycle is the sole persisted Task state authority. V129 archived the
exact old status/configuration fields and removed them from `task`; V130
archived and dropped workflow definitions, transition logs, deferred dispatch,
and task-move storage. Public routes and UI use exact TaskLifecycle transition
receipts, Gate policy revisions/evaluations, and current TaskRole/RoleMembership
records. MCP exposes target lifecycle, Gates, Review Executions/reports,
ValidationRuns, Evidence, roles/memberships, and generic collaboration through
exact identities. Formal Review remains exactly reviewer role plus review
purpose.

PR12 stopped applying historical `ProjectSettings.default_role_assignments`
to newly created Tasks and removed the old state-driven lifecycle script
runner from Execution admission and server startup. V130 preserves the exact
old setting in migration history and removes it from active Project settings;
durable generic Project hooks continue independently from their exact target
records and DomainEvent inputs.

PR12 also separates the public SSE contract from internal `EventContext`.
Typed public events project exact durable DomainEvents and a bounded set of
safe runtime hints; legacy status/workflow, assignment, recovery, Review-row,
and Agent Chat contexts do not escape through SSE. `events.resync_required`
remains a notification after subscriber lag. The EventBus remains a wake and
delivery mechanism, never lifecycle or Gate authority.

### INV-038 — Visibility is not mutation authority

Project read visibility and write authority are separate checks. Owners and
all explicit Project members, including viewers, retain the current read rule;
ownerless system Projects retain public reads. Ordinary Project-derived
mutations require an owner or explicit owner/admin/member membership.
Administrative operations require owner/admin authority. Ownerless visibility
never grants write access, and MCP Project scope only narrows the exact resource
scope; the authenticated Human remains the authority source. Every mutable
Project-derived REST and MCP resource must pass the same operation-appropriate
Project authority boundary.

### INV-039 — Task parent links stay inside one Project

Whenever `Task.parent_task_id` is non-null, the parent and child Tasks belong to
the same exact Project and their `repo_id` values match, including when both
are null. Forge supports one subtask level: a parent is a root, and a Task with
live children cannot itself become a subtask. TaskService rejects absent
parents, cross-Project or cross-Repo links, self-parenting, cycles, nested
parents, and moving a Task that has children. No router or MCP scope check
substitutes for this service invariant.

Every parent-backed create, batch subtask create, attach, reparent, and detach
validates and writes inside one SQLite `BEGIN IMMEDIATE` transaction. This
reserves SQLite's single writer before hierarchy reads, so competing
structural requests revalidate serially. `parent_task_id` and `subtask_order`
change together: attach and moves to another parent append to the destination
sibling order, and detach clears both fields. An ordinary Task update leaves
both fields untouched. A Task with live children cannot be soft-deleted; leaf
deletion is serialized with structural writes. Project teardown deletes its
Tasks and children together through the existing FK cascade. Repo deletion
shares one SQLite `BEGIN IMMEDIATE` boundary between checking whether any Task
references the Repo and mutating the Repo/Project state. If any Task row
references it, deletion is rejected before mutation as `DbError::RepoInUse`,
mapped by REST to `409 Conflict` with `code = repo_in_use`; the Repo,
`Project.primary_repo_id`, and Tasks are preserved. If no Task references it,
REST returns `204`, the Repo is deleted, and `Project.primary_repo_id` is
cleared. Defensive FK/trigger protections may remain, but this preflight and
conflict response are the domain/public contract. This PR does not rewrite
legacy rows; any historical root with a non-null `subtask_order` remains a
PR13 cleanup item if found.

## Current repository baseline

The Plan PR0 audit was performed against the actual clean local checkout:

| Item | Observed value |
| --- | --- |
| Branch | main |
| HEAD | 3d291dd |
| origin/main | 41b4feb |
| Relation | local main is three commits ahead of origin/main |
| Prior audit reference | 41b4febe21cb18e247985d4a8bb2c0c4eb5716c3 |
| Local commits since that reference | 71f478b log rotation, 509205a workflow resume, 3d291dd current-role re-execution |
| Migration head at the Plan PR0 baseline | V085__account_usage_execution_id.sql |
| Workspace state | clean before Plan PR0 edits |

The local commits since the prior audit improve execution logs and recovery.
They do not change the target architecture defined here. Every later Plan PR must
re-check this baseline rather than assuming these refs remain current.

## Target invariants

### INV-001 — Actor is the universal participant abstraction

Every participant capable of task work is an Actor. Initial Actor kinds are
Human and Agent. Humans are not exceptional review callbacks.

### INV-002 — Human and Agent are peers

A Human and an Agent may both plan, implement, review, orchestrate,
communicate, create artifacts, and occupy future roles. UI ergonomics may
differ; domain authority does not.

### INV-003 — Agent is harness-bound

An Agent is a persistent AI Actor bound to a stable harness identity and an
effective HarnessProfileRevision. Harness identity materially affects tools,
interaction, context, editing, approvals, planning, sessions, steering, and
reasoning. An identity-bearing credential or account context is part of the
Agent identity when it changes which native account performs work. Changing
the harness, account, or another identity-bearing property creates another
Agent rather than silently mutating identity. Compatible run configuration may
create a new profile revision on the same Agent. Exact execution
configuration, account context, and capabilities are snapshotted.

### INV-004 — Agent is not Role

PlannerAgent, CoderAgent, ReviewerAgent, and OrchestratorAgent are not domain
classes. Agent is an Actor; Role describes task responsibility.

### INV-005 — Roles are multi-actor

A TaskRole contains zero or more RoleMembership records. A role has a
coordination policy, not one singular assignee. Human and Agent memberships
are both ordinary records. When the replacement TaskRole exists,
RoleMembership is the current authority and the singular legacy assignment is
only a projection; a bounded legacy fallback is valid only while no replacement
TaskRole exists.

### INV-006 — One Actor may hold several roles

The same Actor may hold planner and orchestrator, or orchestrator and reviewer,
on one Task. Each Execution records the role under which it acted.

### INV-007 — Task state does not encode cognition

Task lifecycle state describes aggregate work, never planner thinking,
implementer thinking, reviewer thinking, or another exclusive cognitive turn.
Concurrent activity is represented by Executions and derived projections.

### INV-008 — Execution is the atomic historical work record

Each Execution has exactly one Task, Actor, Role, and Purpose. It may reference
a WorkUnit, HarnessSession, Workspace, parent Execution, configuration and
capability snapshots, usage, outputs, and timestamps. Identity-bearing fields
are immutable after start.

### INV-009 — Execution Purpose is not permission

Purpose answers why an Execution exists. Permission and sandbox policy answer
what it may do. Initial purposes are plan, implement, review, validate,
investigate, orchestrate, and general. Plan is never a permission level.

### INV-010 — HarnessSession is first-class

Agent continuity uses an explicit durable HarnessSession containing Agent,
harness, external session ID, profile and capability snapshots, optional
workspace scope, lifecycle, and timestamps. Role-name or latest-execution
lookup is not the identity mechanism.

### INV-011 — Human execution has no fake HarnessSession

Human work is an Execution with no synthetic harness or session identifier.

### INV-012 — Harness-native capabilities first

When a harness natively supports planning, review modes, resuming, forking,
steering, structured events, model selection, reasoning controls, compaction,
subagents, or approval policies, the adapter uses that capability. Forge does
not duplicate the cognition.

### INV-013 — Capability support is explicit

Capabilities distinguish native, emulated, and unsupported where relevant.
Unknown capability is not supported capability.

### INV-014 — Planning is an Execution

Planning is an Execution with purpose plan. Its output is a generic plan
Artifact produced by the Actor. Planning is not a separate Forge cognition
subsystem.

### INV-015 — No canonical plan engine

Forge does not maintain CanonicalPlan, planner retry cognition, a checklist
authority, or a synchronization loop between a native plan and a Forge plan.
A plan Artifact may be indexed or referenced without becoming a second truth.

### INV-016 — WorkUnit is execution scope

WorkUnit is a concrete piece of executable work. It may be derived from a plan,
but it is not plan truth and does not require bidirectional synchronization.

### INV-017 — Parallel writers have isolated workspaces

Concurrent mutating Executions never share uncontrolled writable authority over
one working tree. Isolated branches/worktrees and explicit integration are
required.

### INV-018 — Reviewer judges work

A reviewer primarily determines whether work is correct and acceptable against
requested criteria. It may inspect, run checks, produce findings, pass, request
changes, or ask questions.

### INV-019 — Orchestrator directs work

An orchestrator determines who should do what and what should happen next. It
may inspect, allocate, route, steer, stop, reassign, escalate, request review,
and request human decisions.

### INV-020 — Orchestrator does not inherently pass or fail implementation

An orchestrator noticing a defect does not create the authoritative review
verdict. A separate reviewer Execution is required for formal review by the
same Actor.

### INV-021 — Orchestrator does not micromanage reasoning

Orchestration operates at work boundaries. The implementation harness remains
autonomous inside its assigned boundary rather than receiving a platform-owned
individual tool-call script.

### INV-022 — Orchestrator is event-driven

Orchestrator Executions awaken on meaningful events, inspect incremental state,
act if needed, and become idle. Continuous token generation while workers run
is not the default model.

### INV-023 — Multiple orchestrators cooperate

A Task may have several orchestrators under a collaborative policy. Durable
Messages, Proposals, and Decisions coordinate them.

### INV-024 — Disruptive actions may require coordination

Stopping, cancelling, reassigning, discarding, invalidating, merging, and
overriding may require Proposal and Decision according to explicit policy.
Read-only and reversible actions do not automatically require consensus.

### INV-025 — Coordination modes are explicit

partitioned means distinct scopes, collaborative means shared coordination,
and independent means deliberately non-contaminating work. The modes are not
encoded in role names.

### INV-026 — Collaboration primitives remain small

The initial collaboration vocabulary is Message, Handoff, Proposal, and
Decision. It is not another general-purpose coordination operating system.

### INV-027 — Artifact is generic

Durable outputs use generic Artifact records. Initial kinds include plan,
review report, validation report, diff, patch, summary, design document,
investigation, API contract, and test report.

The target architecture supports producer references as they are introduced by
their owning Plan PR. PR4 currently supports only an Execution producer through
the normalized relation `artifact_execution_producer`:

~~~text
ArtifactExecutionProducer
  artifact_id -> Artifact
  execution_id -> Execution
~~~

An Execution-produced Artifact derives its Actor from that Execution. PR8 may
add an additive `artifact_validation_run_producer` relation for deterministic,
Actor-free validation output. Artifact does not duplicate `producing_actor`
alongside these references and does not create a fake System Actor for
automated validation. PR4 storage is exactly one of inline content or an
external storage reference; the latter is never a bearer capability.

### INV-028 — Validation and Review are different

Deterministic validation is a core-controlled `ValidationRun`, not an Actor
Execution. A ValidationRun records the check/command identity, bounded
environment/configuration summary, workspace/commit identity, timestamps,
status, exit code, and log/output reference. It does not require a Human,
Agent, HarnessSession, or fake System Actor and produces Evidence plus an
optional generic validation-report Artifact. A Human or Agent may perform
cognitive validation through an ordinary Execution with purpose `validate` or
`investigate`. Review is cognitive judgment recorded by a reviewer Execution.
CI passing does not imply review passing, and review passing does not imply a
ValidationRun ran. Both may be Gates.

### INV-029 — Review feedback is collaboration

Rework is represented by ReviewReport Artifact plus Message or Handoff. Hidden
prompt rewriting is not the durable rework protocol. A PR4 Handoff identifies
an Actor, Role, or Task target and does not persist the recipient's exact
HarnessSession. If PR6 or PR8 requires session continuity for rework, it must
add an explicit additive relation or typed action carrying that identity; it
must never infer continuity from a Role or latest-Execution lookup.

### INV-030 — Deterministic authority stays deterministic

Workspace authority, leases, security policy, human gates, merge constraints,
credential boundaries, and destructive-action policy are core-enforced.
Models cannot reason around them.

Before an external provider Start, the durable claim rechecks the exact open
WorkUnit revision and Actor assignment, active TaskRole membership, and
unexpired WorkspaceLease bound to that Execution in one SQLite write
transaction. An existing provider-attempt receipt short-circuits retries so
an uncertain external effect is never sent again.

An Orchestrator Execution has no WorkUnit or WorkspaceLease. Its claim instead
resolves the exact current OrchestratorWake attempt and, in that same write
transaction, checks the Execution's Task, Agent Actor, `orchestrator` Role,
`orchestrate` Purpose, active TaskRole membership, TaskRole version and policy
snapshot, current Wake state, attempt number, and unexpired Wake lease. A lost
authority claim records no provider attempt. The external attempt receipt is
still checked first so accepted, rejected, and uncertain provider outcomes
cannot be replayed.

A non-repository Agent WorkUnit (`requires_integration = false`) has no
Workspace and receives no WorkspaceLease. Its isolated execution context is
the stable path `.forge-workunit-context/<task_id>/<work_unit_id>` under the
configured workspace root. Parameter preparation and execution resolve that
same exact WorkUnit context. Admission prepares the directory and verifies the
frozen Agent, local CLI adapter, and online embedded host before creating a
Running Execution. A host/provider combination without this local isolated
context is rejected before admission; the missing Workspace never grants
access to the server working directory or the Task integration Workspace.

### INV-031 — Cognitive policy is not Rust workflow branching

Whether to investigate, replan, reassign, select another reviewer, or ask a
human belongs to Actors, explicit policy, and durable collaboration rather than
an oversized hard-coded workflow state machine.

### INV-032 — Unsupported behavior is not silent

If a harness cannot steer an active run, Forge queues the message for a later
turn, performs policy-controlled stop/resume, or reports unsupported. It never
claims native success.

### INV-033 — Historical execution is auditable

Execution history persists Actor, Role, Purpose, Agent, harness, model/profile,
capability, workspace, session, relevant configuration, usage, and outputs
well enough to reproduce the authority and environment of the run. Later
configuration changes do not rewrite history.

### INV-034 — No concurrent hidden sources of truth

Temporary compatibility layers must name the authoritative source, dual-write
direction, bounded lifetime, removal Plan PR, and divergence tests where
practical.

### INV-035 — Schema destruction happens last

The sequence is replacement schema, replacement writes, replacement reads,
legacy-write stop, legacy-read stop, API/UI removal, and only then schema
drop. User data is preserved.

### INV-036 — Documentation follows architecture

No domain-contract Plan PR merges with architecture documentation still describing
the old contract.

### INV-037 — Upstream relationship is attribution

ForgeAILab/forge is the origin. Compatibility is not the goal. Useful
isolated fixes may be selectively adopted, while the independent architecture
and lifecycle are maintained here.

## Pre-PR13 dependency audit

This inventory captures readers and writers before the PR13 replacement Plan PR
deleted or changed them. It is intentionally conservative and remains useful
as pre-cutover evidence; it does not describe the current post-V132 readers.

| Legacy or transitional concept | Current storage and readers/writers observed | Replacement and stop condition |
| --- | --- | --- |
| Singular Task role assignment | V009 task_role_assignment; db TaskRoleAssignmentRepo and sqlite workflow adapter; services TaskService/workflow; API request/response role_assignments; MCP and web task role controls | Plan PR1 adds TaskRole and RoleMembership, makes the membership set authoritative, then Plan PR13 removes singular writes/readers |
| Agent identity/profile and Main/Project bindings | V059 agent_identity, agent_profile, project/account binding records; services agent and chat services; API, MCP, CLI, and web federation surfaces | Plan PR1 introduces ActorRef and harness-bound Agent semantics; Plan PR11 retires Main/Project verticals after consumer migration |
| Execution session continuity | `execution.harness_session_id` references the explicit HarnessSession; `execution.agent_session_id` remains a readable projection only | Plan PR2 adds explicit references; Plan PR10 removes projection-based Resume; Plan PR13 owns physical cleanup |
| Permission and planning mode | executor configuration, CLI adapter settings, historical embedded policy snapshots, workflow/prompt dispatch, and plan-related task paths | Plan PR2 separates Purpose; Plan PR3 maps capabilities and native plan modes; Plan PR7 removes plan permission semantics; PR10 rejects embedded profiles |
| Workflow state machine | V009 workflow_definition/task_state_config; services workflow engine, TaskService transitions, hooks, dispatch loader, recovery, and default workflow; API/UI state controls | Plan PR9 reduces state to aggregate lifecycle and Gates; Plan PR13 removes old workflow persistence after all readers/writers move |
| Special review runtime | review table from V006; crates/review runner/auditor/follow_up; service error and orchestration paths; API/UI review actions; review evidence V084 | Plan PR8 emits validation Evidence and review Executions/Artifacts; Plan PR13 drops special review persistence/runtime |
| Forge-owned cognition | Removed by Plan PR10: no `forge-agent-host` crate, `agent-runtime` dependency, embedded task executor, native tool catalog, or runtime startup wiring. Credential ownership now lives in `CredentialService`; API history readers remain fail-closed. | Complete; protected runtime/session storage remains historical until Plan PR13 |
| Main Agent/Project Agent/Project OS | V061–V076 rooms, chats, bindings, genesis, memory, commitments, attention, charter/baseline, milestones/releases, and related readers/writers | Plan PR11 V118 retires writers, workers, and authority while preserving rows; PR12 removes public surfaces; PR13 drops unused storage |
| Bespoke plan persistence | V081 task_plan_revision/task_plan_approval remain preserved; PR7 stops runtime authority and records V098 provenance migration audit | Plan PR13 owns physical V081 table/column cleanup after all consumers move |
| Project documents and milestone governance | V076 project charter/document/decision/baseline/milestone/release records and orchestration services | Preserve only generic Artifact/Evidence/Gate value; reconcile with the new Task-scoped model during Plan PRs 4, 8, 9, and 11 |
| Workspace and Git isolation | workspace, git, daemon, workspace lease records, execution launch/recovery, and integration paths | Preserve and extend in Plan PR5; no replacement that permits shared writable trees |
| Events and projections | domain_event, events EventBus, SSE routes, attention/mission projections, execution/task event consumers | Plan PRs 4, 6, 9, and 12 update the vocabulary; durable events remain authoritative |

The Plan PR0 baseline migration head was V085. Plan PR0A adds V086 for
authority-aware workspace-lease renewal and V087 for the additive
`cursor_poll` usage source; it edits no historical migration. The table above
is an audit record, not permission to drop any listed table. The Plan PR0A
reconciliation ledger is in [migration/pr-0a.md](pr-0a.md).

The post-PR11 inventory was rechecked at `origin/main` SHA
`815d3ce42af300632056633eed8265f08f82081e`, with migration V118. Main and
Project binding, chat, Project OS, semantic-memory, Attention, and operating
skill writes are fenced at SQLite and retired REST/MCP mutation boundaries.
`project_actor_scope` uses only generic Agent visibility/ownership rules;
Task responsibility and repository leases use TaskRole/RoleMembership and the
exact Execution. Legacy V071 auto-chat triggers are removed. Historical reads
remain for PR12, while physical rows and read-only compatibility are assigned
to PR13. The full writer/reader/authority audit is in
[Plan PR11](plan-pr11-retire-main-project-agent-project-os.md).

## Compatibility policy for the migration

When a replacement representation is needed, the implementation Repo PR for
the owning Plan PR must document these fields in its PR description and
relevant design doc:

1. old writer and new writer;
2. old reader and new reader;
3. authoritative source during the transition;
4. dual-write direction, if any;
5. divergence protection;
6. exact cleanup Plan PR;
7. data-preservation and rollback behavior.

Compatibility is a temporary migration technique, not a target abstraction.
No new alias, deprecated endpoint, v2 suffix, feature flag, or silent
fallback is authorized by Plan PR0.

## Plan PR dependency map

| Plan PR | Additive contract | Legacy readers/writers that remain temporarily | Required cleanup |
| --- | --- | --- | --- |
| Plan PR0 | Documentation, invariants, ADRs, audit | All current paths | Plan PR0A may begin after Plan PR0 is reviewed and merged; Plan PR1 waits for both Plan PR0 and Plan PR0A |
| Plan PR0A | Operational reconciliation: execution log rotation/storage, Cursor large-prompt transport, explicit launch/env configuration, usage/quota observations, WorkspaceLease revision independence, and salvage classification for legacy Repo PR #2 and local commits | Existing singular sessions, workflow cognition, special planning/review paths | Plan PR1 waits until Plan PR0 and Plan PR0A are reviewed and merged; explicit sessions and role migration remain in Plan PRs 1/2 |
| Plan PR1 | ActorRef, TaskRole, RoleMembership, coordination mode | Singular task_role_assignment and Task assignee projection | V127–V128 remove singleton storage after preserving its exact provenance; TaskRole and RoleMembership remain authoritative |
| Plan PR2 | ExecutionPurpose and HarnessSession | execution.agent_session_id projection | V124 archives the exact scalar projection and removes the field; only an explicit HarnessSession reference authorizes Resume |
| Plan PR3 | HarnessAdapter and dimensional capabilities | TaskExecutor supervisor/routing facade; no CodingExecutorAdapter production authority | All harness calls route through adapter; remove the transitional TaskExecutor facade when its remaining generic supervisor consumers migrate |
| Plan PR4 | Artifact, Message, Handoff, Proposal, Decision | Plan/review/chat-specific outputs | Generic records authoritative; remove duplicate outputs in Plan PRs 7, 8, 11 |
| Plan PR5 | WorkUnit DAG and isolated integration | One-task workspace assumptions | WorkUnit isolation is authoritative; Task-scoped recovery/reset cannot infer a Workspace and the shared reset route is removed |
| Plan PR6 | Event-driven orchestrator role and policy | Workflow-triggered cognition | Orchestrator actions use collaboration and small policy; remove orchestration branches in Plan PRs 9/11 |
| Plan PR7 | Plan Execution and plan Artifact | Canonical plan revisions, planner retry/prompt machinery | V125 archives and drops V081 plan persistence; V098 migration provenance remains detached from its source row |
| Plan PR8 | Review Executions, concrete ValidationRuns, and deterministic validation Evidence | ReviewRunner, special review rows, FORGE_RESULT-centric flow | V131 archives and drops special Review persistence; V099 migration provenance remains detached from legacy Review rows |
| Plan PR9 | Aggregate Task lifecycle and Gates | Old workflow engine/state mapping | V129–V130 remove Task status/configuration and workflow persistence after exact state cutover |
| Plan PR10 | External harness cognition, immutable Execution identity, and deterministic no-Workspace admission | agent-host, embedded runtime, and Forge-owned model/tool loop | V126 archives native AgentSession history; V132 archives and drops Agent Chat and retired cognition storage. Task dispatch remains bound to exact Execution snapshots and fails closed without an available host. |
| Plan PR11 | Project/Repo/Task plus generic collaboration | Retired vertical writes/authority are database-fenced; safe history reads remain | V118 retired the authority; V132 archives exact rows and drops legacy runtime storage after PR12 surface removal |
| Plan PR12 | Public REST/MCP/CLI/web/types/SSE projections over target domain | Old API/MCP/CLI/UI and EventBus vocabulary | Removed obsolete surfaces; V121–V132 complete the assigned storage/projection cleanup in this branch |
| Plan PR13 | Destructive persistence cleanup | None if preconditions hold | V121–V132 remove legacy schema and compatibility code after exact-history cutovers; implemented in this branch |
| Plan PR14 | Final product documentation/name | Old public branding where intentionally retained | Rename only with explicit supplied name and data discovery |
| Plan PR15 | Reference scenarios and reliability | None | Final acceptance and documentation correction |

Plan PRs are not combined merely because adjacent code is convenient to edit.
A later Plan PR may be blocked or reordered only by a concrete repository dependency
that is documented and reviewed.

## Plan PR13 implementation status

The current branch applies migrations V121–V134 after the PR12 V120 baseline.
The cutover preserves historical values in dedicated history tables and
migration receipts; those records do not authorize legacy runtime reads or
writes.

| Migration | Cutover |
| --- | --- |
| V121–V122 | Copy exact ProjectRelease snapshot dependencies, references, and media pins into per-release history; detach the historical GET reader from Project OS tables while retaining the public immutable snapshot and shared media authority. |
| V123 | Store Integration `default_implementer` as an exact ActorRef; preserve invalid or ambiguous legacy values in migration provenance without inferring an Actor. |
| V124 | Preserve ambiguous `execution.agent_session_id` values in exact provenance history and drop the scalar projection; target continuity uses an explicitly attached HarnessSession. |
| V125 | Archive V081 Task Plan revisions, approvals, review evidence bundles, and `task.plan`; drop those runtime stores while retaining the V098 migration ledger without its source-row FK. |
| V126–V128 | Preserve native AgentSession and singleton role/assignee projections as history, then drop their runtime stores and Task columns; TaskRole and RoleMembership remain authoritative. |
| V129–V130 | Archive and drop legacy Task status/configuration, workflow definitions, transition logs, deferred dispatch, and task-move storage; TaskLifecycle, Gates, and exact Execution/WorkUnit/Workspace identities remain authoritative. |
| V131 | Archive and drop special Review persistence while retaining detached V099 Review/Validation provenance; current review uses reviewer Executions, ReviewReport Artifacts, ValidationRuns, and Evidence. |
| V132 | Preserve Main/Project Agent, Agent Chat, memory, Attention, Room, Genesis, and Project OS rows in exact legacy-record history, then drop their runtime schema and Project columns. |
| V133 | Add immutable follow-up operation receipts so one idempotency key replays its exact child Execution and cannot authorize different request content. |
| V134 | Replace the WorkUnit Execution admission trigger with distinct rejection markers, preserving exact typed causes for capacity, RoleMembership, WorkUnit, Workspace, dependency, and concurrent-Execution failures. |

ProjectRelease GETs are read-only and reconstruct only from `project_release`,
`project_release_history_*`, shared media assets, and tombstones. They validate
the original `snapshot_digest`; they do not read current Charter, milestone,
readiness, baseline, document, decision, Task, or workflow state.

### PR10 Execution identity and Agent Chat transition

At admission, Forge may resolve an Agent's current daemon binding or select an
available daemon for an unpinned Agent. The resulting daemon ID is stored as
`resolved_daemon_id` in that Execution's immutable config snapshot. Later
Start, Resume, Cancel, graceful shutdown, and same-attempt recovery route
through that ID; they do not inspect the current Agent daemon binding or choose from current daemon
availability order. A host that is missing, malformed, or unavailable causes
a closed failure. Reconnecting the same logical daemon remains valid under
PR3's connection-generation checks. The daemon ID identifies the host chosen
for an Execution; it is separate from a socket connection generation.

On the daemon, each command connection generation owns the Execution tasks
launched through its `DaemonRuntime`, `FallbackExecutor`, and adapter process
maps. When dispatch ends because the socket is lost or the daemon is shutting
down, that runtime closes Start admission, cancels its active tasks through
the same executor and adapters, and waits for bounded task termination before
reconnect. On Unix, each production CLI HarnessAdapter Execution starts in a
dedicated process group whose ID is retained only by its in-memory adapter
handle. Successful adapter cancellation sends SIGKILL to that group, reaps the
leader, and waits until the group no longer exists; a cached leader exit is not
evidence that descendants have exited. Therefore successful retirement proves
that no process in those Execution groups can continue work. Drop guards send a
last-resort group signal but do not prove termination. If a task must be
aborted, or process termination cannot be verified, retirement fails closed.
The same adapter process boundary is used by local and remote HarnessAdapter
execution. On non-Unix hosts, production CLI HarnessAdapter launch fails before
spawning because Forge cannot verify complete process-tree retirement through
the available process-boundary API. This restriction applies to those adapter
paths and does not make every Forge execution path Unix-only; it remains until
Forge can prove that all execution members have exited. The remote connect loop
returns teardown failure to the daemon supervisor, which stops reporting
readiness, shuts down the daemon, and does not create a replacement generation.
A replacement connection for the same daemon ID starts with a new runtime and
never adopts old process handles. If the old socket is already gone, its final
log or terminal notification may be lost; daemon reports and the existing
recovery path reconcile durable Execution state.
The local daemon command also waits for its command-stream task to finish this
retirement after an OS shutdown signal, and propagates retirement failures
instead of reporting a successful stop. It supervises command-stream completion
alongside reporting and stops the reporter if the command loop exits or fails.

Cursor's execution route detection resolves the configured executable without
running `cursor-agent status`. That status command remains part of the separate
detailed availability operation. Neither the server runner nor the remote
daemon starts Cursor's interactive quota poll as an Execution helper: the PTY
child has a separate session/process group that the Execution boundary cannot
verify. Missing Cursor `account_usage` remains unavailable; a prior account
snapshot is not copied into a new Execution.

An Execution's `credential_ref` is immutable invocation identity. Credential
resolution may verify current Agent ownership and credential status, but it
never reads the Agent's current credential to choose a replacement. Secret
material is injected only into the in-memory local adapter invocation. The
generic daemon Start payload does not carry raw credentials; a remote
credential-backed Execution is rejected until a protocol can establish the
same credential identity at the daemon.

Agent Chat persistence is retained: immutable messages, durable turn jobs,
binding/profile provenance, operating-context provenance, retry outcomes, and
historical reads remain available. A Main or Project Chat job does not imply a
model call. Agent Chat has no Task Workspace, and a temporary working
directory or prompt instructions do not establish filesystem isolation. Its
server-owned deny marker is admitted only when the concrete HarnessAdapter
reports both `permission_policy=deny` and `isolation_posture=no-filesystem`,
with no workspace root, custom command override, scoped tools, or MCP
servers. The current production adapter registry cannot prove this posture,
so productive Main/Project Agent Chat model invocation fails closed.

No native typed Forge operations or scoped native tool catalog are available
through these jobs. The generic MCP endpoint is not an Agent Chat tool channel:
it uses an authenticated user and optional Project scope, while the CLI turn
runner does not bind that endpoint to the durable job, profile, or
Main/Project binding. Its descriptor set has generic Project/Task/chat/handoff
operations, but no Main Product Genesis Charter approval protocol and no full
Project document, decision, baseline, milestone, evidence, or readiness
operation set. PR10 does not rebuild the removed tool catalog or add
sandbox/process-isolation infrastructure. PR11 owns retirement or migration
of Main Agent, Project Agent, and Project OS vertical operations; until then,
Agent Chat cannot claim those operations were performed.

## Documentation acceptance questions

The Plan PR0 documents answer these questions:

* An Agent is a persistent AI Actor bound to a stable harness identity and an
  effective HarnessProfileRevision; identity-bearing account context is part
  of that identity when applicable.
* Codex Sol and Cursor Sol are different Agents because the harness changes
  tools, context, approvals, planning, sessions, and execution semantics.
* A Human can implement, plan, review, or orchestrate through ordinary
  Executions.
* Two or more Actors may share a role, and one Actor may hold several roles.
* An orchestrator directs work and routes decisions; a reviewer judges work.
* Multiple orchestrators coordinate with Message, Proposal, and Decision under
  explicit policy.
* Three implementers receive distinct WorkUnits and isolated worktrees.
* WorkUnit is executable scope, not the plan.
* Planning cognition belongs to the native harness or Human planning surface.
* HarnessSession preserves explicit Agent continuity and snapshots its context.
* Review failure creates a review Artifact and rework Handoff; validation and
  review remain separate Gates.
* Workspace, lease, security, credential, merge, and human authority remain
  deterministic in the core.

## Plan PR0 exit report

Important files added or changed:

* docs/architecture.md — target architecture and crate responsibility map;
* docs/migration/architecture-v2.md — this invariant register, audit, and Plan PR
  dependency ledger;
* docs/concepts/ — focused domain contracts;
* docs/adr/ — architecture decisions;
* docs/migration/plan-pr1-preflight.md — audit-only Plan PR1 migration surface;
* docs/upstream.md — independent upstream attribution; the stale downstream
  policy page was removed;
* CLAUDE.md, README.md, docs/api.md, and
  docs/architecture-review-task-actor.md — documentation links or authority
  notices only.

Schema changes: none.

Compatibility storage remaining: the read-only historical
`execution.agent_session_id` projection, native profile/session history, and
the other named pre-PR13 records remain. Plan PR10 removed embedded execution,
runtime startup, and legacy projection-based Resume; Plan PR0 introduced no
compatibility shims.

Deprecated concepts still alive: singular role assignments,
workflow-as-cognition, special review runtime, Main/Project Agent verticals,
and associated legacy persistence. Forge-owned/embedded cognition was retired
in Plan PR10; native profiles and runtime records remain historical, and Plan
PR13 owns their physical cleanup.

Validation: documentation consistency and diff checks are run for this
documentation-only Repo PR. Rust, web, migration, live server, and browser tests
are not required because no production behavior or generated API contract
changes.

Plan PR0A may begin after Plan PR0 is reviewed and merged.

Plan PR1 may begin only after both Plan PR0 and Plan PR0A satisfy their exit
criteria and are reviewed and merged in order.
