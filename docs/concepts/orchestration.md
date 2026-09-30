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

## Implemented subset: PR6

The PR6 runtime claims committed Task-scoped `domain_event` rows and records a
durable wake for the exact active `orchestrator` TaskRole member. Broadcast
events are only low-latency hints. TaskRole coordination mode is captured with
the wake and checked again before dispatch; ambiguous or stale targeting fails
closed. `collaborative` may wake every active member, while `partitioned` and
`independent` dispatch only from explicit allocation or addressing evidence.

Task creation, TaskRole policy/mode changes, and canonical membership writes
also append real durable events inside their SQLite mutation transaction.
An empty TaskRole is not eligible; adding or reactivating a member is the
activation event. This includes compatibility writers because the database
triggers observe the canonical rows. EventBus-only Task notifications are not
authority.

V095 performs a one-time current-state reconciliation for existing, non-
terminal Tasks with eligible orchestrator members. It appends typed
`orchestrator.bootstrap_reconciled` events after the V094 cursor high-water
mark, without recreating historical Task events or replaying the old event
archive. Collaborative targets fan out to active members; independent targets
are materialized only when one active member makes the target unambiguous;
partitioned targets require an exact active Actor allocation to an
orchestrator WorkUnit. The stable role/Actor/WorkUnit dedupe key and the normal
domain-event receipt/wake lease path make migration retry and concurrent
consumers idempotent.

An Agent wake creates a fresh `purpose=orchestrate` Execution and uses a
read-only Harness Start. PR6 does not infer or resume a previous session. A
Human wake stays durable as pending Human work and creates no HarnessSession.
The current typed output set is Message, Handoff, bounded WorkUnit creation,
and Proposal. WorkUnit creation goes through `WorkUnitService`, carries stable
action identity and wake provenance, and does not allocate a workspace or
start work. WorkUnit-scoped actions stay inside that exact WorkUnit. Steering,
stopping, reassigning, merging, and other protected mutations are not
automatically executed by this PR6 action adapter. Proposal and Decision
records remain intent and resolution records with no implicit side effect.
Direct Harness steering has no typed PR6 operation; Message and Handoff record
communication intent without claiming native, emulated, or queued steering.

Execution lifecycle events from an `orchestrate` Execution are classified
separately and never generically wake another orchestrator. Creating a
WorkUnit is an orchestrator action, not a wake signal; completion, readiness,
allocation changes, and explicitly addressed collaboration can wake eligible
members. Unsupported event producers remain deferred to their lifecycle owner
rather than being promoted from EventBus-only signals.

PR6 reads the TaskRole policy using a small versioned schema documented in
[Roles and memberships](roles.md). `{}` keeps the current PR6 defaults.
Unknown fields and versions fail closed. The wake captures the TaskRole version
and exact JSON separately from the fixed runtime policy digest; dispatch and
each action replay require the current TaskRole policy and coordination mode to
match that snapshot. A TaskRole-change event uses the current mode's targeting
rules and does not turn an ambiguous independent or partitioned role update
into a generic fanout.

## Responsibilities

The following responsibility and wake lists describe the target architecture;
the implemented PR6 subset above is narrower.

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

Each wake is a new orchestrate Execution. The target architecture permits
resuming a compatible task-scoped HarnessSession; PR6 always uses Start with a
fresh, Execution-bound HarnessSession. The Execution remains independently
auditable.

## Multiple orchestrators

Several orchestrators may occupy one collaborative TaskRole. They communicate
through Messages and resolve consequential disagreement through Proposals and
Decisions. Disruptive operations are policy-controlled so two orchestrators
cannot endlessly undo each other.

## Steering

The core expresses intent to steer an Execution. The adapter reports whether
steering is native, emulated, queued for a later turn, or unsupported. A
successful queue is not reported as native live steering. Stop/resume fallback
requires explicit policy. PR6 does not expose a direct steering operation;
Messages and Handoffs remain durable communication records only.
