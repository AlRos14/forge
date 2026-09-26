# Orchestration

An orchestrator is an Actor acting under the orchestrator Role. The Role
directs active work across Actors; it is not a super-agent runtime and not a
continuous reviewer.

PR4's generic Message, Handoff, Proposal, and Decision records provide the
initial durable collaboration surface. Their events use the existing
`domain_event` ledger. A Handoff to a Role does not assign a member, and a
Proposal or Decision does not execute the proposed action. Legacy Agent Host
and Project OS records remain authoritative for their own data until PR11;
PR4 does not dual-write them.

## Responsibilities

An orchestrator may:

* inspect Tasks, Roles, WorkUnits, dependencies, Executions, Artifacts,
  Evidence, usage, Messages, and workspace state;
* create WorkUnits and assign or request work;
* request implementations, investigations, validation, and reviews;
* communicate, hand off, steer where supported, pause, stop, resume, or
  reassign subject to deterministic policy;
* create Proposals, record allowed Decisions, and request a Human decision;
* coordinate other orchestrators.

These are typed domain actions. An orchestrator does not receive arbitrary
database mutation or direct repository write authority. If the same Actor
needs to implement, Forge creates a separate implementer Execution.

## Work boundaries

The orchestrator chooses the work boundary and acceptance context. The
implementation HarnessAdapter owns individual tool calls, reasoning, context
management, and native editing behavior. Orchestrator policy must not become a
file-by-file or command-by-command script.

## Event-driven wakeups

Agent orchestrators are idle between meaningful events. Wake events include
Execution start/completion/failure/stall, WorkUnit completion, dependency
satisfaction, Handoff, addressed Message, Proposal resolution, validation or
review failure, merge conflict, quota issue, and Human input. Log-token
streaming is not a wake event by default; implementations may debounce or
coalesce related events.

Each wake is a new orchestrate Execution and may resume the same task-scoped
HarnessSession. The Execution is independently auditable even when the
session continues.

## Multiple orchestrators

Several orchestrators may occupy one collaborative TaskRole. They communicate
through Messages and resolve consequential disagreement through Proposals and
Decisions. Disruptive operations are policy-controlled so two orchestrators
cannot endlessly undo each other.

## Steering

The core expresses intent to steer an Execution. The adapter reports whether
steering is native, emulated, queued for a later turn, or unsupported. A
successful queue is not reported as native live steering. Stop/resume fallback
requires explicit policy.
