# Plan PR9: Gate Engine and aggregate Task lifecycle

Status: **NOT READY FOR REVIEW**. The branch has the additive schema,
aggregate lifecycle, Gate engine, and minimum REST surface, but retry/rework
authority and active WorkflowEngine readers have not been cut over.

## Baseline

- Required base: `326cb240f32c51307a408de225b38340591030d2` (post-PR8 `main`).
- Dedicated branch: `feat/plan-pr9-gate-engine-task-lifecycle`.
- Initial migration head: V099; V100 is the next migration.
- PR8 is merged as repository PR #11 at `326cb240f32c51307a408de225b38340591030d2`.

## Checkpoint 2 — authority inventory

| Classification | Current records and behavior | PR9 disposition |
| --- | --- | --- |
| LEGACY AUTHORITY TO REMOVE | `task.status` is guarded as a projection, but active service paths still consult `WorkflowEngine`/`StateDefinition`/`GateConfig`: task creation/roles (`task_service/create.rs`, `roles.rs`, `claim.rs`), actions/dispatch (`actions.rs`, `task_dispatcher.rs`, `execution/cascade.rs`), and recovery (`execution/recovery.rs`, `services/recovery.rs`). | These reads still affect role selection, dispatch, recovery, and workflow hooks; they are not one-way projections and block PR9 readiness. |
| LEGACY RETRY AUTHORITY | `workflow/actions/gates.rs`, `workflow/actions/merge.rs`, `task_service/execution/recovery.rs`, and diagnostics still expose `max_rejections`, runtime retry budgets, rejection counts, or merge-fix recovery. | A durable exact-failure budget consumer has not replaced these paths. Retry/rework requirements remain open. |
| AUTHORITATIVE | PR8 Review Execution + `review_report` Artifact, ValidationRun + Evidence; PR5 WorkUnit, exact Execution and WorkUnitIntegration; PR6 durable `domain_event`, wake, action and effect receipts; PR7 plan Artifact. | Consume these as independent facts. Do not write their verdicts or merge outcomes from Gate/Lifecycle. |
| COMPATIBILITY | `task.status` is a one-way projection guarded by SQLite; generic legacy Gate approve/reject and Task review approve/reject routes now return `409`; transition URLs can request only finite aggregate lifecycle moves. | Keep the public Task projection until PR12. Do not let workflow state or hooks update lifecycle. Remove public compatibility surfaces in PR12 and physical schema in PR13. |
| HISTORICAL | `task_lifecycle_migration_audit`, legacy `review` rows, V084 review evidence bundles, `review_passed_at`, and prior transition rows remain preserved. | `review_passed_at` and old review rows do not satisfy Gate inputs. Transition-log rejection counts are still read by legacy recovery and therefore are not historical-only yet. |
| IMPLEMENTED IN THIS BRANCH | V100 lifecycle row + migration audit; Gate identity, immutable policy revisions, immutable exact-input evaluations; focused lifecycle and Gate services; durable events and CAS/idempotency; merge source subject consistency and stale-source refusal under the shared workspace lock. | Keep these deterministic facts. Retry/rework and active-reader cutover remain incomplete. |
| PR10+ | Agent-host and embedded cognition/process paths. | No changes. |
| PR12 | Full REST/MCP/CLI/UI projection alignment. | Only the minimum service/domain surface needed for safe lifecycle and Gate calls. |
| PR13 | Physical removal of legacy columns, workflow tables, transition log, review runtime/storage, and compatibility projections. | Preserve data and schemas; make their remaining direction and removal owner explicit. |

### PR9 boundary

Gate is deterministic policy over exact immutable refs. Reviewer Executions
produce ReviewReports; ValidationRuns produce Evidence; Decisions resolve one
exact Proposal; WorkUnit completion and integration remain separate facts.
Orchestrators may request evaluation and act on its result but cannot author a
Gate verdict. Task lifecycle records aggregate progress and cannot represent
planner, implementer, reviewer, validator, or orchestrator activity.

The PR9 compatibility direction is `task_lifecycle -> task.status`. A legacy
status or hook cannot update lifecycle. Existing state/workflow configuration
is retained as historical compatibility data without automatic GatePolicy
conversion. PR12 owns public projection replacement and PR13 owns physical
schema cleanup.

## Invariants

PR9 enforces INV-007, INV-018 through INV-020, INV-022, INV-028 through
INV-031, INV-034, and INV-035. In particular:

- `backlog | ready | active | blocked | ready_to_merge | merging | done | cancelled`
  are aggregate states only.
- Gate satisfaction is an evaluation record, never a mutable boolean.
- Evaluation inputs name exact IDs, versions, digests, producer and subject;
  no query-time `latest` substitution is allowed.
- Same policy revision plus the same normalized exact input set deduplicates to
  one immutable evaluation. New facts create another evaluation.
- A lifecycle effect caused by a GateEvaluation stores that exact evaluation
  reference and an idempotency key in the same transaction as the lifecycle
  row and durable event.
- Retry/rework policy consumes exact failure causes outside Gate evaluation.
- WorkUnit completion is not integration; an integration-required dependency
  must reference its exact successful WorkUnitIntegration.

## New schema and migration mapping

V100 is additive. It adds:

- `task_lifecycle(task_id, state, version, reason_kind, reason_ref, created_at,
  updated_at)` with optimistic version fencing;
- a migration audit that preserves each original status, its mapped lifecycle,
  and any ambiguity/failure provenance;
- `gate` identity scoped to Task, WorkUnit, or an integration/merge operation;
- immutable, versioned, schema-versioned, digestible `gate_policy_revision`;
- immutable `gate_evaluation` and ordered `gate_evaluation_input` rows;
- lifecycle transition receipts that bind the causal GateEvaluation (when
  present), idempotency key, before/after versions, and durable event.

No old migration is edited. Initial mapping is conservative:

| Legacy status | Lifecycle state | Audit |
| --- | --- | --- |
| `backlog` | `backlog` | exact mapping |
| `todo`, `ready` | `ready` | exact mapping |
| `planning`, `in_progress`, `working` | `active` | aggregate work only; cognitive distinction discarded |
| `blocked` | `blocked` | preserve legacy reason when available |
| `done` | `done` | terminal history preserved |
| `cancelled` | `cancelled` | terminal history preserved |
| `review` | `blocked` | fail closed; old state does not prove ReviewReport, required Validation, reviewer quorum, or Decision |
| `merging` | `merging` only with one exact active TaskMerge operation; otherwise `blocked` | operation identity is preserved; ambiguous in-flight state fails closed |
| `merge_failed` | `blocked` | retain exact legacy failure/transition provenance when reconstructible |
| custom/unknown | `blocked` | preserve raw status and workflow reference; do not invent policy |

Legacy status is updated to its documented projection when a lifecycle
transition commits. Migration preserves the original value in its audit row.
Unknown state names, policy schemas and hook actions fail closed.

## Gate policy v1

The normalized schema is deliberately bounded and versioned. It supports
all-required inputs plus a bounded review quorum (`one_of`, `all`, `at_least`),
an optional Human requirement, and explicit ActorRefs or a frozen TaskRole and
membership revision. Validation requirements pin check identity,
configuration digest, Workspace, commit, snapshot digest and required outcome.
Decision requirements pin Proposal ID/content version, Decision ID/outcome,
policy reference/version/digest and permitted decider ActorRefs. WorkUnit
requirements pin WorkUnit/version, dependency state, Execution result, and the
exact WorkUnitIntegration when required. Unknown fields/schema versions cannot
produce a satisfied evaluation.

CI and review remain independent requirements. A ReviewReport PASS never
substitutes for a ValidationRun PASS. A Decision is authorized only when its
exact Proposal version, policy snapshot and decider match. Gate inputs must be
same-Task and match the Gate's exact scope.

## Lifecycle, retry, merge, and event ownership

`TaskLifecycleService` owns allowed aggregate transitions, lifecycle and Task
version fencing, causal identity, idempotency, projection update, and the
durable event transaction. Gate-caused movement requires the exact persisted
satisfied evaluation; replay uses that evaluation and cannot recalculate it
against newer facts.

Retry budgets are not Gate inputs. The target is a lifecycle/orchestration
policy that consumes exact failure facts (ReviewReport request-changes,
ValidationRun failure, Execution failure, or merge conflict), deduplicates each
failure identity, blocks on exhaustion, and directs Handoff/WorkUnit/new
Execution while budget remains. **That consumer is not implemented in this
branch.** Today an unsatisfied request-changes evaluation remains a durable Gate
fact, but it does not consume a retry budget or create rework. This is a P2
readiness blocker.

Gate evaluation is driven from durable fact events and policy revision events;
EventBus is only a post-commit hint. Evaluation persistence and its event are
idempotent and replay-safe. A crash after evaluation but before lifecycle
movement leaves the evaluation available for replay. A crash after lifecycle
movement returns the stored transition receipt and does not dispatch or merge
again.

Merge readiness is a satisfied merge Gate followed by `ready_to_merge`, then
atomic admission to `merging` through the existing TaskIntegrationOperation
serialization and MergeService. Successful exact merge completion moves to
`done`; conflict/failure moves to `blocked` with its exact operation reference.
Before admission, MergeService reads the exact evaluation inputs under the
shared workspace lock. ReviewReport and ValidationRun subjects must agree on
Workspace, commit, and snapshot; direct merge requires a completed source
Execution in that Workspace; WorkUnit-backed merge requires an exact
successful WorkUnitIntegration whose target workspace and commit still match
the merge source. A stale source is rejected before creating the merge
operation. PR9 does not reimplement workspace isolation, leases, merge locks,
or recovery.

## Relationship to earlier Plan PRs

- PR5 remains authoritative for WorkUnit scope, dependency DAG, isolated
  Workspace/Lease, completion result, TaskIntegrationOperation, and exact
  integration outcome.
- PR6 remains authoritative for durable event claims, orchestrator wake
  snapshots, typed actions, effect guards and receipts. Orchestrators react to
  facts but cannot write Gate outcomes.
- PR8 remains authoritative for Review Execution/ReviewReport and
  ValidationRun/Evidence. Gate reads their exact frozen identity and never
  creates a second verdict or check result.

## Focused verification plan

Focused tests currently cover migration mapping/audit, policy immutability and
version fencing, Task-scoped Gate REST behavior, exact validation-to-merge
readiness, stale merge-source refusal before admission, lifecycle
concurrency/idempotency, GateEvaluation event replay, and rejection of the
legacy merge entry point. The full reviewer quorum, Decision, WorkUnit,
stale-input, merge-failure/recovery, and retry-budget matrices are not closed.
This acceptance evidence gap is a separate P2 readiness blocker.

## Validation results

Commands run on this branch:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS |
| `git diff --check` | PASS |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db --test file_backed_migration pr9_migration_maps_legacy_task_states_conservatively_and_audits_ambiguity --locked --offline` | PASS, 1 test |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_lifecycle::tests --locked --offline` | PASS, 5 tests |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib merge_service::tests --locked --offline` | PASS, 1 test |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test gate_engine_api --locked --offline` | PASS, 2 tests |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test happy_path --locked --offline` | PASS, 2 tests |
| `pnpm --dir web typecheck` | PASS |

No workspace-wide suite was run. These passes do not close the reviewer,
Decision, WorkUnit, stale-input, merge-failure/recovery, or retry-budget
acceptance gaps listed above.

## Deferred cleanup and limits

PR9 does not drop `task.status`, workflow definitions, state configs,
`transition_log`, `blocked_json`, `entry_barrier_json`, old review storage,
or API/MCP/UI compatibility surfaces. Every remaining reader/writer must be
bounded to a compatibility projection or historical data; a path able to
change lifecycle or certify a Gate is a PR9 defect. PR12 owns complete public
surface alignment; PR13 owns destructive schema cleanup. PR10, PR11, PR14 and
PR15 remain out of scope.

## Checkpoints

- CHECKPOINT 1 — PR8 merged and ancestry verified at `326cb240f32c51307a408de225b38340591030d2`.
- CHECKPOINT 2 — authority inventory and PR9 boundary recorded above.
- CHECKPOINT 3 — V100 schema/lifecycle implemented; focused lifecycle and migration tests pass.
- CHECKPOINT 4 — Gate policy/evaluation and Task-scoped REST paths implemented; review + validation Gate smoke and stale merge-source refusal pass.
- CHECKPOINT 5 — durable GateEvaluation replay and lifecycle receipts have focused coverage; the API harness explicitly runs the event consumer.
- CHECKPOINT 6 — partial: `task.status` is a one-way projection, but live WorkflowEngine/retry readers remain and block readiness.
- CHECKPOINT 7 — commands and exact results are recorded in Validation results. Broader reviewer/Decision/WorkUnit/retry and merge recovery matrices remain incomplete.
- CHECKPOINT 8 — P1=0; P2=3: missing exact-failure retry/rework consumer, active WorkflowEngine/GateConfig readers, and incomplete requested acceptance matrix. PR9 is NOT READY FOR REVIEW.
