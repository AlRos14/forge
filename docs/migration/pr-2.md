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
row without Purpose and rejects Human Agent/session compatibility fields.

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
exact legacy Agent id. Agentless or ambiguous history is not advertised as
resumable. API responses and operator/display projections retain old fields but
expose the new additive fields.

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
is not resumable.

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

## Historical backfill

`V089__execution_purpose_and_harness_sessions.sql` is additive:

* `agent_id IS NOT NULL` maps exactly to `actor_kind = agent` and
  `actor_id = agent_id`.
* Agentless historical rows are not guessed from current Task assignment or
  RoleMembership. They remain unresolved and receive an
  `historical_actor_unresolved` migration issue.
* Historical Purpose uses only the persisted historical role: planner -> plan,
  reviewer -> review, coder/worker/implementer/executor/merge_fixer ->
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
session references, and post-start Actor/Role/Purpose/attached-session
changes. Failed or
cancelled Executions do not automatically end a reusable HarnessSession.

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
* conservative historical Actor/Purpose/session backfill and ambiguity issues.

Service unit coverage covers the exact role-purpose compatibility mapping and
the action-resolver distinction between pending, active explicit, Agent legacy,
and historical legacy session state.

## Validation actually executed

* `cargo fmt --all`, targeted `cargo check` commands, and compile-only test
  targets were executed before the final fixture/schema edits and before the
  explicit instruction to avoid Cargo/Rust validation. No Cargo, rustc, or
  Rust test command was run after that instruction or after the final edits.
* The focused PR2 DB test target was attempted but cannot run on the current
  merged PR1 baseline because V088 itself fails on a fresh SQLite migration
  with `near "AS": syntax error` in trigger-body `UPDATE ... AS ...` syntax.
  PR2 does not edit V088 by contract. This pre-existing migration-chain
  blocker remains `REQUIRES IMPLEMENTATION-TIME VERIFICATION` and is recorded
  in the preflight and final report.
