# Independent orchestration architecture: Plan PR0 contract

Status: accepted as the migration target for this repository.

This document freezes the architecture before production code is changed. It
is a design contract and migration ledger, not a claim that the target model
is already implemented.

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
| Migration head | V085__account_usage_execution_id.sql |
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
are both ordinary records.

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

Artifact provenance uses one mutually exclusive producer reference:

~~~text
ArtifactProducer
  Execution(execution_id)
  ValidationRun(validation_run_id)
~~~

An Execution-produced Artifact derives its Actor from that Execution. A
ValidationRun-produced Artifact is deterministic and Actor-free. Artifact does
not duplicate `producing_actor` alongside these references and does not create
a fake System Actor for automated validation.

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
prompt rewriting is not the durable rework protocol. Exact implementer session
continuity is used when possible.

### INV-030 — Deterministic authority stays deterministic

Workspace authority, leases, security policy, human gates, merge constraints,
credential boundaries, and destructive-action policy are core-enforced.
Models cannot reason around them.

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

## Current dependency audit

This inventory identifies current readers and writers before any replacement
Plan PR deletes or changes them. It is intentionally conservative; each later Plan PR
must repeat the search at its own HEAD.

| Legacy or transitional concept | Current storage and readers/writers observed | Replacement and stop condition |
| --- | --- | --- |
| Singular Task role assignment | V009 task_role_assignment; db TaskRoleAssignmentRepo and sqlite workflow adapter; services TaskService/workflow; API request/response role_assignments; MCP and web task role controls | Plan PR1 adds TaskRole and RoleMembership, makes the membership set authoritative, then Plan PR13 removes singular writes/readers |
| Agent identity/profile and Main/Project bindings | V059 agent_identity, agent_profile, project/account binding records; services agent and chat services; API, MCP, CLI, and web federation surfaces | Plan PR1 introduces ActorRef and harness-bound Agent semantics; Plan PR11 retires Main/Project verticals after consumer migration |
| Execution session continuity | execution.agent_session_id and executor snapshots; cli-adapters emit external session IDs; services launch, cascade, recovery, follow-up, context manifests, and embedded execution consume them | Plan PR2 adds HarnessSession and explicit references; compatibility backfill/read ends in Plan PR13 |
| Permission and planning mode | executor configuration, CLI adapter settings, embedded execution policy, workflow/prompt dispatch, and plan-related task paths | Plan PR2 separates Purpose; Plan PR3 maps capabilities and native plan modes; Plan PR7 removes plan permission semantics |
| Workflow state machine | V009 workflow_definition/task_state_config; services workflow engine, TaskService transitions, hooks, dispatch loader, recovery, and default workflow; API/UI state controls | Plan PR9 reduces state to aggregate lifecycle and Gates; Plan PR13 removes old workflow persistence after all readers/writers move |
| Special review runtime | review table from V006; crates/review runner/auditor/follow_up; service error and orchestration paths; API/UI review actions; review evidence V084 | Plan PR8 emits validation Evidence and review Executions/Artifacts; Plan PR13 drops special review persistence/runtime |
| Forge-owned cognition | forge-agent-host crate, agent-runtime dependency, embedded_agent_service, embedded_task_executor, native/typed tools, startup wiring in forge-cli/api/services | Plan PR10 extracts legitimate credential/process infrastructure, migrates adapters, then removes agent-host |
| Main Agent/Project Agent/Project OS | V061–V075 rooms, chats, bindings, genesis, memory, commitments, attention, project charter/baseline, and related services/routes/MCP/UI | Plan PR11 moves useful durable behavior onto generic actors/collaboration/artifacts, then Plan PR12 removes obsolete surfaces |
| Bespoke plan persistence | V081 task_plan_revision/task_plan_approval; plan capture routes, services, UI, review binding, and prompt builders | Plan PR7 treats plan output as Artifact and removes canonical-plan authority after readers/writers move |
| Project documents and milestone governance | V076 project charter/document/decision/baseline/milestone/release records and orchestration services | Preserve only generic Artifact/Evidence/Gate value; reconcile with the new Task-scoped model during Plan PRs 4, 8, 9, and 11 |
| Workspace and Git isolation | workspace, git, daemon, workspace lease records, execution launch/recovery, and integration paths | Preserve and extend in Plan PR5; no replacement that permits shared writable trees |
| Events and projections | domain_event, events EventBus, SSE routes, attention/mission projections, execution/task event consumers | Plan PRs 4, 6, 9, and 12 update the vocabulary; durable events remain authoritative |

The current migration head is V085. Plan PR0 adds no migration and edits no
historical migration. The table above is an audit record, not permission to
drop any listed table.

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
| Plan PR1 | ActorRef, TaskRole, RoleMembership, coordination mode | Singular task_role_assignment | New membership authority; remove old role path in Plan PR13 |
| Plan PR2 | ExecutionPurpose and HarnessSession | execution.agent_session_id and inferred resume paths | Explicit session authority; remove compatibility in Plan PR13 |
| Plan PR3 | HarnessAdapter and dimensional capabilities | TaskExecutor/CodingExecutorAdapter facade | All harness calls route through adapter; remove facade when consumers finish |
| Plan PR4 | Artifact, Message, Handoff, Proposal, Decision | Plan/review/chat-specific outputs | Generic records authoritative; remove duplicate outputs in Plan PRs 7, 8, 11 |
| Plan PR5 | WorkUnit DAG and isolated integration | One-task workspace assumptions | WorkUnit isolation authoritative; remove shared assumptions in Plan PR13 |
| Plan PR6 | Event-driven orchestrator role and policy | Workflow-triggered cognition | Orchestrator actions use collaboration and small policy; remove orchestration branches in Plan PRs 9/11 |
| Plan PR7 | Plan Execution and plan Artifact | Canonical plan revisions, planner retry/prompt machinery | Artifact authoritative; remove old plan APIs/persistence in Plan PR13 |
| Plan PR8 | Review Executions, concrete ValidationRuns, and deterministic validation Evidence | ReviewRunner, special review rows, FORGE_RESULT-centric flow | Generic review/validation authoritative; remove special runtime in Plan PR13 |
| Plan PR9 | Aggregate Task lifecycle and Gates | Old workflow engine/state mapping | New lifecycle authoritative; remove workflow tables/branches in Plan PR13 |
| Plan PR10 | External harness cognition only | agent-host and embedded runtime | Remove crate/dependency/startup consumers |
| Plan PR11 | Project/Repo/Task plus generic collaboration | Main/Project Agent and Project OS verticals | Remove old services/tables after migration fixtures |
| Plan PR12 | Public surfaces over target domain | Old API/MCP/CLI/UI endpoints | Remove obsolete endpoints and UI |
| Plan PR13 | Destructive persistence cleanup | None if preconditions hold | Drop old schema and compatibility code |
| Plan PR14 | Final product documentation/name | Old public branding where intentionally retained | Rename only with explicit supplied name and data discovery |
| Plan PR15 | Reference scenarios and reliability | None | Final acceptance and documentation correction |

Plan PRs are not combined merely because adjacent code is convenient to edit.
A later Plan PR may be blocked or reordered only by a concrete repository dependency
that is documented and reviewed.

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

Compatibility shims remaining: all pre-existing runtime paths remain in place;
Plan PR0 introduces none.

Deprecated concepts still alive: singular role assignments, inferred execution
session continuity, workflow-as-cognition, special review runtime,
Forge-owned/embedded cognition, Main/Project Agent verticals, and associated
legacy persistence. Their current readers/writers and removal Plan PRs are listed
above.

Validation: documentation consistency and diff checks are run for this
documentation-only Repo PR. Rust, web, migration, live server, and browser tests
are not required because no production behavior or generated API contract
changes.

Plan PR0A may begin after Plan PR0 is reviewed and merged.

Plan PR1 may begin only after both Plan PR0 and Plan PR0A satisfy their exit
criteria and are reviewed and merged in order.
