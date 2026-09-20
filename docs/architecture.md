# Architecture

This document is the architectural source of truth for the independent
orchestration system Forge is becoming. It deliberately supersedes the
previous Main Agent, Project Agent, embedded cognition, singular-assignee, and
workflow-as-cognition model.

Forge originated as a fork of [ForgeAILab/forge](upstream.md). The origin,
license, and attribution remain important; architectural compatibility with
upstream does not. The migration is intentionally incremental. At Plan PR0 the
Rust, SQLite, REST, MCP, CLI, and web implementations still contain the
previous model. They remain operational until a later Plan PR moves their
readers and writers. This document describes the target contract, not a claim
that every target primitive already exists.

The migration register, invariant definitions, current dependency audit, and
Plan PR boundaries live in [migration/architecture-v2.md](migration/architecture-v2.md).
The focused domain references are in [concepts/](concepts/).

## North star

> The orchestrator owns work. The harness owns cognition.

Forge is a local-first work orchestration platform in which humans and AI
agents are peer actors. The platform owns durable work, authority,
collaboration, workspaces, evidence, and lifecycle. A harness owns its native
model interaction loop, tools, context handling, planning behavior, steering
semantics, and session protocol.

Forge coordinates at work boundaries. It must not become a second model runtime,
a canonical planning engine, or a reviewer disguised as an orchestrator.

## Responsibility boundaries

| Responsibility | Owner |
| --- | --- |
| Actor identity, task scope, roles, memberships, executions, work units, gates, evidence, and durable collaboration | Forge core |
| Deterministic authorization, leases, workspace isolation, merge policy, required checks, credential boundaries, and idempotency | Forge core |
| Model cognition, native planning/review modes, context loop, harness tools, model steering, and harness session semantics | Harness and HarnessAdapter |
| Human decisions and human work performed through the control plane | Human Actor using the same domain primitives |
| Git operations, worktree creation, locking, and integration mechanics | git and workspace infrastructure |
| Process discovery, daemon transport, provider invocation, and usage observation | forge-daemon, cli-adapters, and harness adapters |

The boundary is semantic, not merely a crate boundary. A helper remains
platform-owned when it enforces authority or records durable state. A helper is
harness-owned when it decides how a model reasons or how a native harness
conducts its interaction loop.

## Domain model

~~~text
Project
├── Repo*
└── Task*
    ├── TaskRole*
    │   └── RoleMembership*
    ├── WorkUnit*
    ├── Execution*
    │   ├── Actor
    │   ├── Role
    │   ├── Purpose
    │   ├── HarnessSession?
    │   └── Workspace?
    ├── ValidationRun*
    ├── Artifact*
    ├── Gate*
    ├── Evidence*
    └── Collaboration
        ├── Message*
        ├── Handoff*
        ├── Proposal*
        └── Decision*

Actor
├── Human
└── Agent
    ├── Harness identity
    ├── Credential/account context?
    └── HarnessProfile
        └── HarnessProfileRevision
            ├── Model
            └── Configuration
~~~

Every durable record is scoped to the owning account and, where applicable,
Project and Task. Opaque IDs are references that still require authorization;
they are never authority by themselves.

## Actors, Agents, and harnesses

An Actor is the universal participant abstraction. The initial kinds are Human
and Agent. Both can plan, implement, review, orchestrate, communicate, create
artifacts, and occupy several roles at once.

An Agent is a persistent AI Actor bound to a stable harness identity. The
harness is part of the Agent's identity because it changes tools, interaction
loop, context handling, editing strategy, approvals, planning, sessions, model
steering, and reasoning environment. Two records such as GPT-5.6 Sol in Codex
and GPT-5.6 Sol in Cursor are therefore different Agents even if their model
labels match. An identity-bearing credential or account context is also part
of the Agent identity when it changes which native account performs work.

A `HarnessProfileRevision` tunes future runs: model, reasoning effort,
approval policy, sandbox settings, and non-identity harness arguments. A
compatible configuration change may create a new profile revision on the same
Agent. Changing the harness, account, or another identity-bearing property
creates another Agent rather than silently mutating identity. Each Execution
snapshots the exact effective profile, account context, and capabilities used.
See [actors.md](concepts/actors.md) and
[agents-and-harnesses.md](concepts/agents-and-harnesses.md).

The platform exposes capability support dimensionally. A capability is
native, emulated, or unsupported where that distinction matters. Unknown
support is not support. The core never advertises a read-only sandbox, prompt
convention, or fallback process as native harness planning or steering.

## Roles and memberships

Roles describe responsibility for one Task; they do not classify Actors and do
not create Agent subclasses. Initial role names are planner, implementer,
reviewer, and orchestrator, but persistence must not hard-code behavior solely
from a string.

A TaskRole has a coordination policy and zero or more RoleMembership records.
Membership records participation only. They do not own a WorkUnit, path,
concrete scope, or current assignment. WorkUnit allocation and historical
attempts belong to WorkUnit and Execution, so an Actor can remain a member
while moving between WorkUnits without rewriting membership history.
The policies are:

| Mode | Meaning | Typical use |
| --- | --- | --- |
| partitioned | Members work on distinct scopes | Multiple implementers |
| collaborative | Members share state and coordinate decisions | Multiple orchestrators |
| independent | Members deliberately evaluate without influencing one another | Multiple reviewers or competing planners |

The same Actor may occupy planner and orchestrator, or orchestrator and
reviewer. Each Execution records the role under which the Actor acted. Human
membership is ordinary domain data, not a special approval callback.

See [roles.md](concepts/roles.md).

## Executions

An Execution is the atomic historical work record. It has exactly one Actor,
one Role, one Purpose, and one Task. A WorkUnit, HarnessSession, Workspace,
parent Execution, configuration snapshot, capability snapshot, usage, output,
status, and timestamps are associated explicitly where applicable.

Initial purposes are:

~~~text
plan | implement | review | validate | investigate | orchestrate | general
~~~

Purpose answers why the Execution exists. Permission and sandbox policy answer
what it may do. They are separate dimensions. plan is never a permission
level.

Execution identity-bearing fields become immutable once work starts. Parent
lineage records causal relationship; it does not imply session reuse. A Human
Execution has no fake HarnessSession. An Agent Execution may have an explicit
HarnessSession, but a session is never guessed from a role name or latest
Execution.

See [executions.md](concepts/executions.md) and
[sessions.md](concepts/sessions.md).

## Deterministic validation runs

Core-controlled checks are not Actor cognition and do not require an Actor,
Agent, HarnessSession, or fake System Actor. They are represented by a
`ValidationRun` that records the Task/WorkUnit or related Execution context,
check or command identity, bounded environment/configuration summary,
workspace and commit identity, lifecycle timestamps, status, exit code, and a
log/output reference. A ValidationRun produces Evidence and may produce a
generic validation-report Artifact.

An Actor may still perform cognitive validation work—such as investigating a
failed test, reproducing a bug, or interpreting security output—through an
ordinary Execution with purpose `validate` or `investigate`. That Execution
does not turn deterministic checks into Actor-owned work. Validation and
review remain independent Gate inputs.

## Planning and WorkUnits

Planning is an ordinary Execution with purpose plan. A planner Actor uses
the native planning capability of its harness, or a Human uses the planning
surface. The result is persisted as a generic Artifact of kind plan.

There is no canonical Forge plan engine. Forge may render, index, validate
references to, and authorize access to a plan Artifact, but it does not
continuously synchronize a second internal plan truth. Multiple plans may
coexist. Choosing one is an explicit Gate, Decision, or orchestration action.

A WorkUnit is a concrete executable scope. It may be created manually, by an
orchestrator, from a plan Artifact, from an issue, or from another WorkUnit.
It is not the plan and does not require bidirectional plan synchronization.
WorkUnit dependencies form a bounded DAG and make scheduling constraints
explicit. See [work-units.md](concepts/work-units.md).

## Harness sessions

HarnessSession is a first-class durable entity for Agent continuity. It records
the owning Agent, harness kind, external session identifier, profile and
capability snapshots, optional workspace scope, lifecycle, and timestamps.
Executions attach to a session explicitly.

Rework targets the exact Actor and, when possible, resumes the exact
HarnessSession. A review Execution may be related to an implementation
Execution without sharing its session. Restart/recovery operates on persisted
session metadata rather than reconstructing identity from role labels.

The platform must never silently emulate a harness-native capability. If live
steering is unsupported, the intent is queued for a future turn or materialized
through a policy-controlled stop/resume operation, and the result says so.

## Collaboration

The initial durable collaboration vocabulary is intentionally small:

* Message — communication between Actors, Roles, or a Task scope.
* Handoff — a transfer or request of work or responsibility.
* Proposal — a proposed consequential action.
* Decision — the resolution of a Proposal.

These records may reference a WorkUnit, Execution, Artifact, or Gate, but they
do not become a second authority hierarchy or a social messaging product.
Reviewer feedback becomes a ReviewReport Artifact plus a Handoff or Message,
not hidden prompt rewriting. Two orchestrators coordinate through the same
primitives. See [collaboration.md](concepts/collaboration.md).

## Orchestration

An orchestrator directs work. It inspects current state, allocates WorkUnits,
requests and routes Executions, manages dependencies, communicates, steers
where supported, asks for human decisions, and coordinates other
orchestrators. It does not inherently produce the authoritative review
verdict, and its tools do not grant arbitrary database mutation or direct
repository write access.

If the same Actor must implement and orchestrate, Forge creates separate
Executions under the two roles. An orchestrator operates at work boundaries,
not individual tool calls inside an autonomous implementation harness.

Orchestrator Executions are event-driven. A task-scoped Agent orchestrator
awakens on meaningful events, inspects incremental state, acts if needed, and
returns idle. It does not consume model tokens continuously while workers run.
Several orchestrators may be active under a collaborative policy.

Operations are classified as observational, soft/reversible, or
operational/disruptive. Stop, cancel, reassign, discard, invalidate, merge,
and override actions may require a Proposal and Decision according to the
TaskRole policy. Consensus is not globally hard-coded; the policy is explicit.

See [orchestration.md](concepts/orchestration.md).

## Review, validation, and gates

Validation is deterministic evidence from ValidationRuns: tests, typechecks,
lint, builds, security scanners, and required commands. Actor-driven
validation is ordinary cognitive work recorded as an Execution. Review is
cognitive judgment by a reviewer Actor. Neither validation form implies that
review passed, and review never implies that a deterministic check ran.

A reviewer inspects work, evidence, and criteria, then produces a generic
review Artifact with findings and a verdict. A reviewer does not become an
orchestrator merely because it noticed a defect. An orchestrator routes that
defect; a formal review requires a separate reviewer Execution.

Gates express deterministic constraints such as required validation, required
reviewers, human authorization, merge readiness, or a policy decision. A Gate
can require several independent, partitioned, or collaborative reviewers.
Review failure produces rework collaboration and may resume the original
implementer's exact session.

See [review-and-validation.md](concepts/review-and-validation.md) and
[gates.md](concepts/gates.md).

## Artifacts and evidence

Artifact is the generic durable output primitive. Initial kinds include plan,
review report, validation report, diff, patch, summary, design document,
investigation, API contract, and test report. Artifacts can be content or an
external/path reference with metadata, digest, Task, and timestamps. Their
producer is an explicit mutually exclusive `ArtifactProducer`:

~~~text
ArtifactProducer
  Execution(execution_id)          # Actor is derived from the Execution
  ValidationRun(validation_run_id) # deterministic, Actor-free producer
~~~

An Execution-produced Artifact derives its Actor, Role, Purpose, harness and
configuration provenance from that Execution. A deterministic validation
Artifact derives provenance from its ValidationRun and never requires an
Actor, HarnessSession, or fake System Actor. This avoids duplicating
`producing_actor` as a second source of truth.

Evidence is a typed, auditable observation used by a Gate or lifecycle
projection. Deterministic validation can produce Evidence and a validation
Artifact. A cognitive reviewer can produce a review Artifact that references
Evidence. Bespoke artifact subsystems require a later domain justification.

See [artifacts-and-evidence.md](concepts/artifacts-and-evidence.md).

## Task lifecycle and authority

Task state describes aggregate work lifecycle, not which cognitive role owns
the turn. The target default vocabulary is:

~~~text
backlog → ready → active → blocked → ready_to_merge → merging → done
                                                     ↘ cancelled
~~~

The exact enum can evolve during the migration, but states must not mean
planner thinking, coder thinking, or reviewer thinking. Planning, implementing,
reviewing, validating, and orchestrating may overlap. UI activity such as
Planning, three Implementers active, Reviewing, or Waiting for Human is a
derived projection.

The scheduler starts eligible requested Executions while respecting dependency
edges, Actor capacity, WorkUnit scope, workspace leases, Gates, Task lifecycle,
and Decision policy. Cognitive questions such as whether to investigate,
replan, reassign, or ask another reviewer belong to Actors and explicit policy,
not a growing Rust branch tree.

Deterministic authority remains in the core. Models cannot reason around
workspace leases, security policy, human gates, merge constraints, credentials,
or destructive-action policy.

## Workspace isolation and integration

Concurrent mutating Executions never receive uncontrolled write authority over
the same working tree. The target topology is:

~~~text
Task integration branch/workspace
├── WorkUnit A branch/worktree
├── WorkUnit B branch/worktree
└── WorkUnit C branch/worktree
~~~

Each mutating parallel Execution gets an isolated branch/worktree and an
explicit lease. Integration into the Task branch is deterministic, locked,
observable, and separate from work-unit completion. Merge conflicts and failed
integration are explicit operational state and Evidence, never hidden Git
accidents.

The scope text on a WorkUnit is coordination context. It is not a filesystem
security boundary unless an actual path-level authority is implemented.

The existing workspace and git crates are preserved as infrastructure and will
be evolved behind this contract.

## Persistence and migration rules

The replacement is additive until the final cleanup Plan PR:

1. Add replacement schema and types.
2. Write the replacement representation.
3. Read the replacement representation.
4. Stop legacy writes.
5. Stop legacy reads.
6. Remove legacy APIs and UI.
7. Drop legacy schema only after migration fixtures prove preservation.

Any compatibility reader must name the authoritative source, its bounded
lifetime, the dual-write direction if applicable, and the Plan PR that removes it.
Historical uncertainty is preserved as unknown; migrations do not invent
harness semantics, role intent, or approvals.

All migrations are numbered and preserve user data. Historical migrations are
immutable. SQLite row mapping, repository traits, services, API types,
generated TypeScript, routes, MCP tools, CLI commands, event consumers, and
documentation move together when a public contract changes.

## Crate responsibilities

The target responsibility map is:

| Crate | Target responsibility | Migration direction |
| --- | --- | --- |
| forge-daemon | Process discovery, daemon transport, harness process lifecycle, usage observation | Preserve and evolve |
| cli-adapters | Harness-specific adapter implementations and protocol translation | Preserve implementation, move behind HarnessAdapter |
| workspace | Worktree creation, leases, path safety, cleanup | Preserve and extend for WorkUnits |
| git | Low-level Git operations and deterministic integration | Preserve |
| config | Configuration precedence and local data paths | Preserve |
| api-types | Shared public request/response/domain types | Thin and align to new domain |
| db | Additive persistence, repositories, migrations, row mapping | Thin around new primitives |
| services | Deterministic orchestration domain, scheduling, authority, projections | Replace cognitive workflow branches with small domain services |
| api | Thin REST/SSE boundary and authentication/authorization mapping | Thin |
| forge-client | Operational CLI client | Rebuild public commands later |
| mcp-server | MCP projection of the same domain primitives | Thin, no second domain |
| events | Durable-event projection and live delivery | Preserve, update event vocabulary |
| executors | Transitional execution/logging facade | Replace with adapter-facing execution boundary |
| review | Transitional special review runtime | Reduce to validation utilities, then remove if no longer meaningful |
| agent-host | Forge-owned cognition/runtime layer | Remove after credential/process infrastructure is extracted |
| forge-cli | Startup and composition root | Thin after retired services/workers are removed |

Target dependency flow:

~~~text
forge-cli → api → services → db
                → events
          → mcp-server ─────┘
          → HarnessAdapter → cli-adapters → forge-daemon
          → workspace → git
          → config
          → api-types
~~~

agent-host and any embedded cognition runtime are not in the target graph.
They remain in the current build until Plan PR10 removes them.

## Public surfaces

REST, MCP, CLI, SSE, and the web UI expose the same Actor/Role/Execution/
WorkUnit/Artifact/Evidence/Collaboration model. They must not introduce
different authority or orchestration concepts. A public contract change is
updated in the route, api-types, generated TypeScript, and docs/api.md in one
implementation Repo PR for the owning Plan PR.

The UI may make Human work more ergonomic, but Human planner, implementer,
reviewer, and orchestrator actions use the same domain records as Agent work.
The UI shows Agent harness identity clearly; it does not collapse same-model
Agents across harnesses.

Plan PR0 does not change the product runtime, public API, CLI surface, or
branding. It corrects current API-reference text where necessary so it does
not describe unimplemented behavior; new public domain surfaces remain staged
for later Plan PRs.

## Migration sequence

The migration is one reviewed Plan PR at a time:

| Plan PR | Contract |
| --- | --- |
| Plan PR0 | Freeze this architecture, invariant register, ADRs, upstream attribution, and migration ledger |
| Plan PR1 | Actor references and multi-actor TaskRole memberships |
| Plan PR2 | ExecutionPurpose and first-class HarnessSession |
| Plan PR3 | Capability-driven HarnessAdapter |
| Plan PR4 | Generic Artifacts and collaboration primitives |
| Plan PR5 | WorkUnits and isolated parallel implementation |
| Plan PR6 | Event-driven, multi-actor orchestration |
| Plan PR7 | Harness-native planning Executions and plan Artifacts |
| Plan PR8 | Review Executions, concrete ValidationRuns, and deterministic validation Evidence |
| Plan PR9 | Aggregate Task lifecycle and simplified gates/scheduler |
| Plan PR10 | Removal of agent-host and embedded cognition |
| Plan PR11 | Retirement of Main Agent/Project Agent/Project OS verticals |
| Plan PR12 | REST, MCP, CLI, web, and event surface alignment |
| Plan PR13 | Legacy persistence and compatibility cleanup |
| Plan PR14 | Final product rename and documentation rewrite |
| Plan PR15 | Reference scenarios, reliability hardening, and acceptance |

No later Plan PR is implied by a Plan PR0 document. Each Plan PR begins from the actual
current main, re-runs dependency searches, identifies affected invariants,
and stops after its own validation and review.

## Related documents

* [Architecture v2 migration contract](migration/architecture-v2.md)
* [Actors](concepts/actors.md)
* [Agents and harnesses](concepts/agents-and-harnesses.md)
* [Roles](concepts/roles.md)
* [Executions](concepts/executions.md)
* [Harness sessions](concepts/sessions.md)
* [WorkUnits](concepts/work-units.md)
* [Collaboration](concepts/collaboration.md)
* [Orchestration](concepts/orchestration.md)
* [Review and validation](concepts/review-and-validation.md)
* [Artifacts and evidence](concepts/artifacts-and-evidence.md)
* [Gates](concepts/gates.md)
* [Upstream relationship](upstream.md)
* [Architecture decision records](adr/)
