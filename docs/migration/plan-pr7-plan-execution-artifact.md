# Plan PR7 — Planning Execution and generic Plan Artifact

Status: ready for independent review on
`feat/plan-pr7-plan-execution-artifact`, based on
`27acb8cb1096eda099dcbc1a233d5e0251f1aece`. This branch is not merged.

## Authority

Planning is an ordinary `Execution` with `purpose=plan`. Its persisted
`ActorRef` is the author. Completion creates one immutable
`Artifact(kind=plan)` on the same Task, with its producer Execution recorded by
the generic `artifact_execution_producer` relation. A Human uses a real
`ActorRef::Human(user_id)` Plan Execution with the planner TaskRole and no
Agent, HarnessSession, or synthetic System identity.

The durable output key is `(execution_id, kind)` in
`execution_artifact_output`. It is generic for Execution output Artifacts. The
insert and Artifact/event creation share one SQLite transaction. A retry with
the same Task, kind, storage, content, metadata, digest, and producer returns
the existing Artifact; a different result for that Execution fails. Competing
connections serialize on the database unique key, so at most one output
identity and one `artifact.created` event commit.

Downstream dispatch carries exact Artifact ids. The generic
`execution_artifact_input` relation pins the same-Task id and digest before
dispatch records a log path. Re-execution and follow-up inherit the parent's
exact Plan Artifact inputs; a Plan Execution contributes its own output.
Prompts may include content for convenience, but no downstream consumer
queries “the latest plan” to decide its input. Later Plan Executions create new
Artifacts and cannot rewrite previous outputs or inputs.

The Markdown parser produces display-only checklist items, progress, and
warnings. No checkbox gates Task transitions, execution, or dispatch. WorkUnit
may refer to an exact Artifact using the existing generic provenance but does
not mirror or synchronize plan content.

## V081 history migration

V098 leaves `task_plan_revision` and `task_plan_approval` intact. Planning
runtime no longer reads or writes them. For each source Execution with a
single unambiguous content digest, V098 migrates the preferred
`planner_ready` checkpoint only when the exact same-Task Execution has
`purpose=plan`, a persisted Human or Agent ActorRef, and that Actor identity
still exists. Existing matching generic Plan outputs are bound/reused rather
than copied again. Conflicting or ambiguous pre-existing Plan outputs remain
unbound and are recorded as migration conflicts.

Other V081 checkpoints for the same source Execution and digest map to that
one Artifact. In particular, an `approved` checkpoint does not change the
content author or create a second Artifact. An approved checkpoint with no
source Execution maps to an already verified same-Task Plan Artifact only when
the digest matches; the approver remains approval history, not Artifact
producer.

Rows without recoverable provenance remain in V081 and receive an explicit
status in `legacy_task_plan_artifact_migration`; they have no Artifact id and
no fabricated Actor. The audit records the old revision id, checkpoint,
source Execution id, digest, status, and mapped Artifact id where applicable.
The V081 approval table itself is preserved unchanged. Migration is additive
and runs transactionally.

After V098, generic Plan Artifacts are the only planning source of truth.
There is no dual-write. The physical V081 tables and the physical
`task.plan` column remain until PR13, which owns their eventual removal after
data-retention and dependent-surface checks. `Task.plan` has been removed from
runtime models, repository create/update inputs, Task serializers, and new
write SQL; old column values are preserved but not read as plans.

## Removed planning authority and compatibility remnants

The PR7 scan and changes cover these paths:

- `plan_artifact.rs` no longer captures files into V081, reads V081 history, or
  chooses an authoritative latest revision. It is a Markdown projection over
  generic Artifacts only.
- `task_plan_revision` and `task_plan_approval` have no planning, gate, or
  review evidence writers. The default planning state is Active; its approval,
  rejection loop, plan-specific awaiting-human metadata, and checklist gate are
  removed. Workflow resolution filters the retired checklist hook from
  previously stored workflow definitions so it cannot block a transition as an
  unknown action. Generic gates remain owned by the transitional workflow
  until PR9.
- `Task.plan` is removed as a repository/service/API/MCP writer and from the
  runtime Task model. The `forge_update_task` schema has no `plan` property.
  Legacy values in the physical column are retained but never used as
  downstream context.
- Dispatch no longer falls back from an Artifact to `Task.plan`. It includes
  only the Plan Execution output or Plan Artifact inputs pinned on the exact
  causing Execution. Review evidence names the exact Plan Artifact ids/digests
  already supplied to its Execution; legacy `plan_revision_id` and digest
  columns remain unused compatibility storage until PR13.
- `PlannerPromptBuilder`, the `planner.default.v2` builder, the required
  `../plan.md` script, `write_plan`, `PLAN_ARTIFACT_AGENT_INSTRUCTION`,
  `FORGE_RESULT plan_ready`, planner result persistence, and planner-specific
  `Retry Planning`/rejection handling are removed. The HarnessAdapter capability declaration
  decides whether planning is native/emulated/unsupported. Claude Code
  selects its native Plan mode; explicit `emulated` capability remains
  allowed; `unsupported` and `unknown` fail before dispatch without silent
  fallback.
- The V082 TaskDecisionRequest planner producer is removed with the old result
  parser. Historical request rows remain readable/answerable as records, but
  answering one no longer starts a planner Execution. New Actor questions use
  generic Handoffs. Broader removal of the old decisions API is deferred to its
  owning public-surface cleanup.
- `/api/v1/tasks/{id}/plan` remains temporarily as a read-only projection over
  up to 100 newest generic Plan Artifacts. It returns Artifact identity,
  producer, digest, content, timestamp, and derived display data; it contains
  no revision or approval authority. Task response plan summary fields are
  derived from the generic Artifact projection. PR12 owns broader UI/API
  consolidation.
- `PlanDocument` and `PlanChecklist` remain display components. They show
  Artifact/producer identity and derived content; checklist progress does not
  control lifecycle.

Project OS documents, Project Agent planning, milestone/baseline planning, and
their product-genesis flows remain under PR11. This change does not remove
those surfaces merely because their names contain “plan”. The generic Task
planner/Project OS distinction remains explicit.

## Harness and Human paths

Agent Plan dispatch has `purpose=plan` before the HarnessAdapter is invoked.
Planning invocation and capability evidence are explicit. The successful
assistant result is captured in full and materialized directly; the harness is
not given a Forge-authored tool-call script. If recovery finds an already
materialized output for that Execution, it completes from that Artifact
without running another model turn. A successful local or remote Plan result
without complete assistant output fails rather than consulting a file or
generating a second protocol.

`CollaborationService::start_human_plan_execution` requires the real Human to
hold the Task's planner Role and creates a running Human Plan Execution.
`complete_human_plan_execution` validates that exact Actor and Execution,
creates/reuses the Plan Artifact, and completes the same Execution. A caller
must not create an Agent id or HarnessSession to represent the Human. A public
Human planning editor is not introduced here; that surface is owned by PR12.

## P2 independent-review fixes

Creating a Running Execution with initial Artifact inputs now inserts the
Execution, validates and pins every same-Task Artifact with its stored digest,
and appends `execution.started` in one SQLite transaction. Initial dispatch,
re-execution, follow-up, cascade retry, and recovery pass their exact selected
Artifact ids into that operation. An invalid input rolls back the Execution,
all input rows, and the start event. Other SQLite connections see the complete
input set whenever they can see the committed start event. The EventBus publish
remains post-commit. Repository-backed dispatch keeps the existing
WorkspaceLease check before returning to the caller and starting the runner;
lease issuance itself remains in its existing service flow.

Remote terminal notifications now carry `assistant_output` separately from
the bounded `summary`. The optional serde-defaulted field keeps older daemon
payloads readable. A remote Plan Execution requires non-empty full output,
commits or reuses its exact Plan Artifact, and only then records the terminal
Execution status and event. A crash after Artifact materialization leaves the
Execution Running; replay with identical output reuses the `(execution_id,
kind)` output binding without another Artifact or creation event. Different
replay content fails closed. An older daemon's completed Plan notification
without full output leaves the Execution Running and creates no Artifact.
Non-Plan remote completion does not require `assistant_output`.

Focused microfix validation used the shared Cargo target. The transaction
boundary test, both remote Plan completion tests, old-daemon deserialization,
generated binding check, and daemon output-producer test passed. A supplementary
`subtask_sequence_guard_rejection_runs_orchestrator_instead_of_coder_follow_up`
filter failed at its existing `lease-backed subtask follow-up exists`
assertion. Its fixture has no resumable `HarnessSession`, so it exits through
the no-session guard before the Artifact-input retry caller; it does not
exercise either P2 invariant. No broader suite was run.

## PR13 cleanup inventory

PR13 may remove the physical V081 `task_plan_revision` and
`task_plan_approval` tables, their indexes and immutable triggers, the V098
legacy migration audit table after its retention/audit use ends, the old
`task.plan` column, review evidence `plan_revision_id`/`plan_digest` columns,
and compatibility-only decision-request tables/routes after their owner has
migrated consumers. PR13 must preserve legacy history until its explicit data
retention and migration preconditions are satisfied; PR7 does not drop tables.

## Validation

Focused validation after the requested Cargo cleanup:

- PASS — `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p db pr7_migration_preserves_legacy_plan_history_and_maps_only_verifiable_authorship -j 2` (V098 final actor-provenance trigger and legacy mapping).
- PASS — `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p services plan -j 2` (19 tests, including Agent/Human provenance, immutable revisions, exact downstream pinning, cross-connection idempotency, no V081 writes, no checklist gate, and removal of the bespoke planner retry exception).
- PASS — `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p services workflow_resolution_removes_retired_plan_checklist_hook -j 2` (one test; stored workflows cannot reactivate checklist blocking).
- PASS — `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p executors planning_requires_an_explicit_native_or_emulated_capability -j 2` (native/emulated accepted, unsupported/unknown fail closed).
- PASS — `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p cli-adapters planning -j 2` (2 Claude native planning tests).
- PASS — `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0 cargo test -p api --test planning_gate -j 2` (3 tests; generic Gate continuation preserves the exact Human-authored Artifact, creates no duplicate, and writes no V081 approval; an initial unseeded-user fixture failure was corrected before the passing rerun).
- PASS — `pnpm --dir web exec vitest run src/components/__tests__/PlanDocument.test.tsx src/components/__tests__/PlanChecklist.test.tsx` (2 files, 6 tests), `pnpm --dir web typecheck`, and `pnpm --dir web generate:types` (completed before the requested Cargo cleanup; no web source changed afterwards).
- PASS — `rustfmt --edition 2021 --check` on modified Rust files and `git diff --check`.
- NOT RUN — full workspace tests and release build. `cargo clean` was run earlier at the user's request, removing 43,548 files and reclaiming 40 GiB; all subsequent Cargo commands used the existing `/home/alejandro/Proyectos/forge/target` with debug symbols and incremental compilation disabled.
