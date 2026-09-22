# Plan PR2 — Implementation / Exit Ledger

## Status

Plan PR2 implementation is on `feat/plan-pr2-execution-session` from the
merged Plan PR1 baseline. PR2 is additive and does not merge or remove the
legacy embedded Agent Runtime session vertical.

## Old writer -> new writer

All production `CreateExecution` paths now provide a concrete `ActorRef` and
an explicit `ExecutionPurpose` through the existing TaskService/review
semantic paths. Claim, manual launch, initial dispatch, follow-up, recovery,
re-execute, cascade/workflow continuation, reviewer/auditor, and failed-record
creation are covered. The database admission path rejects a new actor-bearing
row without Purpose, rejects Human Agent/session compatibility fields, and
rejects the reserved legacy `human` sentinel as a new Actor principal.

Agent creation persists `ActorRef::Agent(agent_id)` and projects the same id to
`execution.agent_id`. Human claim creation persists the concrete user handle
as `ActorRef::Human(user_id)` with `agent_id` and all harness-session fields
NULL. No production writer persists the legacy `"human"` sentinel.

## Old reader -> new reader

Resume, follow-up, cascade continuation, blocked recovery, action resolution,
review continuation, and executor-result handling now consult
`Execution.harness_session_id` first, then the referenced HarnessSession and
its lifecycle/external identity. The old `execution.agent_session_id` value is
only a bounded fallback for historical rows without a generic reference, an
exact persisted Agent identity (including the V089 ActorRef backfill), and an
exact legacy Agent id with compatible workspace evidence. Agentless, sentinel,
pending, or ambiguous history is not advertised as resumable. API responses
and operator/display projections retain old fields but expose the new additive
fields. Re-execute remains a new Execution and records the old Execution as
`parent_execution_id`; it does not implicitly reuse its HarnessSession.

Manual session follow-up passes the chosen Execution as causal lineage only;
the current RoleMembership selects the new Actor before the explicit session
reuse check. A membership change therefore creates a new/no session instead of
inheriting the previous Actor's continuity.

Blocked-session recovery and workflow-guard retry use the same Actor-first
order. They select the current usable RoleMembership Agent when that
authoritative row exists; only a matching Actor may reuse the blocked or
completed Execution's explicit HarnessSession. An Actor change creates the
new Execution with the current Agent snapshot and no inherited session.

## Execution principal authority

The physical PR2 representation of `Execution.actor_ref` is
`execution.actor_kind` plus `execution.actor_id`. It is immutable after insert.
`execution.agent_id` is an Agent-only compatibility projection and is checked
against the Actor for new Agent rows. Human rows have no Agent projection.

## Purpose authority

`ExecutionPurpose` contains exactly `plan`, `implement`, `review`, `validate`,
`investigate`, `orchestrate`, and `general`. Runtime writers state the purpose
at the semantic call site: planning/review/validation/investigation task types
map to `plan`/`review`/`validate`/`investigate`; normal implementation and
review-fix/cascade rework map to `implement`; orchestration maps to
`orchestrate`; and generic interactive work maps to `general`. The role mapping
helper is only a small fallback for legacy role-driven paths and compatibility
recovery. Historical migration uses the deterministic role-only mapping.
Purpose is not permission. The current executor `permission_policy: plan`
behavior remains transitional and is not aliased to `ExecutionPurpose::Plan`.

## Session authority and compatibility direction

The generic `harness_session` table records Forge identity, Agent ownership,
opaque harness kind, external harness session id, profile/capability snapshots,
optional workspace scope, predecessor, timestamps, and the
`pending`/`active`/`ended`/`failed` lifecycle. Agent and harness identity are
immutable; external identity cannot be changed once known. A pending session
is not resumable. PR2 only pre-materializes pending continuity for the current
executor families that can report an external session identity; known
non-session executors do not receive a fabricated session, and an unknown
opaque harness can materialize one when its result supplies an identity.

Explicit continuity also requires the Execution's snapshotted harness kind to
match the HarnessSession's opaque harness identity. A profile change within
that same harness keeps the original session snapshot; a harness mismatch
fails closed rather than crossing continuity boundaries.

`workspace_id` on a HarnessSession is a historical scope token rather than a
foreign key to the replaceable operational `workspace` row. Workspace reset or
deletion therefore cannot silently turn a workspace-scoped session into
unscoped continuity; current-workspace checks fail closed until an explicitly
compatible session is selected. A predecessor must belong to the same Agent
and harness identity and cannot be cleared by cascading deletion.

The one-way compatibility projections are:

```text
Execution.actor_ref
    |
    +--> execution.agent_id                    (Agent-only projection)

Execution.harness_session_id
    -> HarnessSession
    -> HarnessSession.external_session_id
    |
    +--> execution.agent_session_id            (legacy projection)
```

`Execution.executor_config_snapshot_json` remains intact and distinct from
the HarnessSession profile/capability snapshot. Result persistence updates
the generic external identity, status/activity, and legacy projection in one
transaction where practical. Repeating a result for the same Execution does
not create another generic session.
Repository updates that request terminal snapshot cleanup preserve the
Execution snapshot while an explicit HarnessSession is attached, so a failed
or cancelled run remains reconstructible for resume.

## Historical backfill

`V089__execution_purpose_and_harness_sessions.sql` is additive:

* Non-sentinel `agent_id IS NOT NULL` maps exactly to `actor_kind = agent` and
  `actor_id = agent_id`. The reserved legacy `agent_id = 'human'` sentinel is
  not converted into `ActorRef::Agent`; it remains unresolved compatibility
  data.
* Agentless and sentinel historical rows are not guessed from current Task
  assignment or RoleMembership. They remain unresolved and receive an
  `historical_actor_unresolved` migration issue.
* Historical Purpose uses only the persisted historical role: planner -> plan,
  reviewer/auditor -> review, coder/worker/implementer/executor/merge_fixer ->
  implement, orchestrator -> orchestrate, and all other roles -> general.
* Existing rows with the same Agent, harness kind, and external session id
  share one HarnessSession when profile/workspace evidence is coherent.
* Contradictory historical profile/workspace evidence is not merged. Rows stay
  on bounded legacy compatibility and receive a
  `historical_session_ambiguous` issue.

The migration never rewrites old principal identity from current membership or
current Agent configuration.

## Atomicity and divergence protection

Fresh session materialization is inside the existing Execution/claim
transaction. Result-time session materialization, activation, and the legacy
projection update share the Execution transaction. SQLite guards reject actor
projection mismatch, Human session fields, incompatible Agent/workspace
session references, invalid pending/active lifecycle transitions, empty
external identities, cross-identity predecessors, legacy projection
divergence, and post-start Actor/Role/Purpose/attached-session changes. A
cancelled Execution with an attached HarnessSession retains its executor
snapshot so a later recovery does not advertise continuity that it cannot
reconstruct. The repository has one external-identity writer: the transactional
Execution result path for new runtime work; migration and explicit historical
materialization are the bounded exceptions. Failed or cancelled Executions do
not automatically end a reusable HarnessSession, and Human result callbacks
fail closed.

Crash A (pending session committed before process death) remains visibly
pending and is not advertised as resumable. Crash B (external harness session
exists but Forge has not persisted its identity) is not guessed or remotely
discovered; the session remains unresolved under the existing recovery policy.

## Legacy embedded AgentSession boundary

V062 `agent_session`, `agent_context_scope`, protected runtime state,
`AgentSessionRepo`, context manifests, and embedded Agent Host/runtime paths
remain a separate compatibility vertical. An embedded runtime may project its
`runtime_session_id` into the generic HarnessSession through the normal
Execution result path, but generic callers do not depend on Agent Host
session semantics.

## Exact cleanup ownership

* Plan PR3: `HarnessAdapter` and explicit native/emulated/unsupported
  capability semantics.
* Plan PR7: remove plan-as-runtime/cognition compatibility.
* Plan PR10: remove or extract embedded Agent Runtime and legacy AgentSession
  behavior.
* Plan PR12: final REST, API, MCP, CLI, and UI surface cleanup.
* Plan PR13: remove `execution.agent_id` and `execution.agent_session_id`
  compatibility where the target permits, remove inferred resume fallbacks,
  tighten/drop transitional nullable columns, and perform destructive
  persistence cleanup.

## Rollback and data preservation

Rollback is additive at the schema level: old columns remain, historical
rows are not deleted, and the legacy embedded session tables are untouched.
The generic session and Execution references use historical-preserving FK
behavior; a referenced HarnessSession cannot be deleted through the schema.
Reverting application code does not erase V089 records or legacy projections;
forward reconciliation remains available before PR13.

## Tests added

`crates/db/tests/pr2_execution_session.rs` covers:

* Agent/Human principal persistence and fake-human rejection;
* explicit Purpose and role/Purpose independence;
* pending creation, activation, result projection, idempotent retry, and
  external identity immutability;
* explicit Actor/session/workspace reuse constraints;
* Agent and harness collision domains;
* Actor/Purpose/HarnessSession immutability and profile snapshot preservation;
* conservative historical Actor/Purpose/session backfill, sentinel rejection,
  and ambiguity issues;
* pending/active lifecycle protection and Human external-result rejection.

The MCP execution projection has a focused regression test for additive
`actor_ref`, `purpose`, and `harness_session_id` output.

Service unit coverage covers the exact role-purpose compatibility mapping and
the action-resolver distinction between pending, active explicit, Agent legacy,
and historical legacy session state.

## Validation actually executed

* This review pass ran static source/migration audits and `git diff --check`
  only. Per instruction, no Cargo, rustc, Rust test, or frontend build command
  was run after the final edits.
* An earlier implementation pass attempted the focused PR2 DB target before
  the no-Rust instruction; it was blocked on the merged PR1 V088 migration
  chain (`near "AS": syntax error` in trigger-body `UPDATE ... AS ...`
  syntax). PR2 does not edit V088 by contract. The implementation therefore
  remains `REQUIRES IMPLEMENTATION-TIME VERIFICATION`.
