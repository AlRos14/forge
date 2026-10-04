# Plan PR9: Gate Engine and aggregate Task lifecycle

Status: **READY FOR REVIEW**. PR9 is not merged. The merge-readiness decision
is a deterministic evaluation of immutable exact facts; Task lifecycle stores
aggregate progress. The PR8 ReviewReport and ValidationRun authority remains
unchanged.

## Baseline and checkpoints

- Required base: post-PR8 `main` at `326cb240f32c51307a408de225b38340591030d2`.
- PR8 HEAD: `834b6c486b56c9abbf63fe26248eacacea0e0d74`; ancestry verified in `main`.
- PR9 branch: `feat/plan-pr9-gate-engine-task-lifecycle`.
- PR9 initial HEAD: `6367cc257f9a6ccc1febcce9b4ab6abefd4161d8`.
- Migration head at PR9 base: V099. PR9 adds V100–V107; each migration is additive.
- PR8 was merged as repository PR #11; resulting `main` is the PR9 base above.

### CHECKPOINT 1 — PR8 merged

Old `main` was `b4001c86d6fafbddcffdf0bf0b1bbd605be2ea9e`; PR8 HEAD was
`834b6c486b56c9abbf63fe26248eacacea0e0d74`; merge commit/resulting `main` was
`326cb240f32c51307a408de225b38340591030d2`. The PR8 HEAD is an ancestor of
the post-merge `main`. PR9 started at that exact `main`.

### CHECKPOINT 2 — PR9 authority inventory

| Classification | Current authority or record | PR9 disposition |
| --- | --- | --- |
| AUTHORITATIVE | Review Execution and ReviewReport Artifact; ValidationRun and Evidence; Proposal and exact Decision; WorkUnit, Execution, WorkUnitIntegration; durable DomainEvent | Gate consumes exact facts. Reviewers, validators, and orchestrators do not author Gate outcomes. |
| NEW AUTHORITY | Immutable GatePolicyRevision, GateEvaluation, ordered GateEvaluationInput, TaskLifecycle and transition receipt | Deterministic policy and aggregate progress now own readiness and Task lifecycle. |
| COMPATIBILITY | `task.status`, finite legacy Task transition route, workflow-definition read/configuration views | `task.status` is a one-way SQLite-guarded projection of TaskLifecycle. Legacy Task transition input maps to aggregate states only. No workflow hook writes lifecycle. Public projection alignment remains PR12. |
| HISTORICAL STORAGE | `transition_log`, `blocked_json`, `entry_barrier_json`, `review_passed_at`, workflow/state definitions, GateConfig, old review rows | Retained as historical/configuration data. Retry counts, old review state, and GateConfig do not authorize a Gate or lifecycle transition. Physical removal belongs to PR13. |
| BOUNDED COMPATIBILITY | Workflow resolution used by workflow read/configuration and prompt-preview surfaces; crate-private legacy recovery/cascade code | WorkflowEngine transition entrypoints are no longer public outside `services`; three unused mutation entrypoints were removed. Retired recovery actions are rejected at the public service boundary. Remaining config/read projections belong to PR12; physical engine/table cleanup belongs to PR13. |
| PR10+ | Embedded agent/host cognition and process ownership | Unchanged. |
| PR12 | Full REST/MCP/CLI/UI lifecycle and Gate projection | Only minimum REST Gate/lifecycle surface is present here. |
| PR13 CLEANUP | Legacy columns/tables, WorkflowEngine implementation, ReviewRunner/storage, old transition rows | Preserve source data until the destructive cleanup plan. |

### PR9 boundary

PR9 replaces lifecycle and Gate authority; it does not add a cognitive
workflow. Reviewer Executions produce ReviewReports. ValidationRuns produce
Evidence. Decisions resolve an exact Proposal. Orchestrators react to durable
facts and may direct rework, but cannot declare a Gate satisfied. WorkUnit
completion and integration remain separate facts. Workspace leases, serialized
TaskIntegrationOperation admission, Git merge, and crash recovery remain the
existing PR5/PR6 infrastructure.

## Invariants

PR9 enforces INV-007 and the applicable collaboration, exact-provenance,
version-fencing, and event-replay invariants referenced by
`architecture-v2.md`.

- Lifecycle states are `backlog`, `ready`, `active`, `blocked`,
  `ready_to_merge`, `merging`, `done`, and `cancelled`. They describe aggregate
  work progress, not planning, implementation, review, validation, or
  orchestration cognition.
- Gate satisfaction is an immutable evaluation, never a mutable boolean.
- Policy revisions are immutable, normalized, versioned, schema-versioned,
  digestible, and fail closed for unsupported schemas.
- Evaluation inputs bind exact IDs, versions, digests, producer and subject.
  Evaluation never substitutes a latest review, validation, or approval.
- A repeated policy revision and exact input set deduplicates to one durable
  evaluation. A new input set or policy revision creates a separate record.
- SQLite fences merge-readiness lifecycle writes against the active policy,
  latest evaluation for that revision, exact satisfied outcome, and current
  mutable input versions. It also fences exact TaskMerge admission and
  terminal outcomes in the same transaction.
- Lifecycle transitions use Task and lifecycle versions, a causal identity,
  an idempotency key, an immutable transition receipt, and a durable event.
  Replaying a receipt returns its original evaluation and does not repeat an
  effect.
- Retry/rework consumes exact failure identities outside Gate policy.
- A completed WorkUnit does not imply integration. Integration-required
  dependencies pin the exact WorkUnitIntegration.

## Schema and migration mapping

V100 adds `task_lifecycle`, migration audit, Gate identity, immutable
`gate_policy_revision`, immutable `gate_evaluation`, exact ordered
`gate_evaluation_input`, and lifecycle transition receipts. It adds database
guards for Task scope, exact merge admission, satisfied merge readiness, and
successful/failed TaskMerge lifecycle effects.

V101 adds durable exact-failure retry receipts. V102 fences TaskRole membership
changes with version increments and durable events, and makes receipt teardown
follow Project deletion. V102 also requires TaskRole policy/coordination edits
to advance the version fence. V103 rejects direct Ready/Active lifecycle writes
after the exact retry budget is exhausted. V104 rechecks mutable exact Gate
inputs at lifecycle and merge-effect boundaries. V105 allows a
`ready_to_merge` Task to return to `active` from either a new exact
GateEvaluation or the retry policy's exact durable rework receipt. A later
lifecycle transition supersedes that receipt for lifecycle effects, so replay
cannot reopen work after a subsequent readiness or user decision. Arbitrary
Actor/status transitions remain fenced. V106 serializes Running Execution
admission against terminal lifecycle writes: an insert or resume must observe
the durable TaskLifecycle in the same SQLite write transaction, so a launch
that loses a cancellation/done/merge race cannot leave a Running Execution on
a terminal Task.

V107 binds the `interactive` Execution label to the TaskRole selected by the
Task operation type whenever that canonical role exists. SQLite rejects a
WorkspaceLease insert or renewal whose Agent is not an active member; the
pre-TaskRole singleton fallback remains available only when the role is absent.

The existing `task.status` column is preserved and normalized as a lossy
projection. The migration audit preserves the original state, workflow
definition, configuration, and relevant failure details.

| Legacy status | New lifecycle | Audit behavior |
| --- | --- | --- |
| `backlog` | `backlog` | Exact mapping. |
| `todo`, `ready` | `ready` | Exact mapping. |
| `planning`, `in_progress`, `working` | `active` | Aggregate work only; old cognitive distinction is discarded. |
| `blocked` | `blocked` | Preserve legacy reason fields. |
| `done`, `cancelled` | Same | Preserve terminal history. |
| `review` | `blocked` | `ambiguous_review`; status alone proves no ReviewReport, Validation, quorum, or Human Decision. |
| `merging` | `merging` only with its exact active TaskMerge operation; otherwise `blocked` | Preserve operation identity or fail closed as ambiguous. |
| `merge_failed` | `blocked` | Preserve exact failure/transition provenance when reconstructible. |
| Custom/unknown | `blocked` | Preserve raw value and workflow data; do not invent policy. |

New Task rows with unsupported legacy labels also fail closed to `blocked` and
retain an audit reason. No automatic conversion from WorkflowDefinition,
StateDefinition, GateConfig, or hooks creates a GatePolicy.

## Gate policy and exact inputs

Gate policy v1 is intentionally bounded. It supports all-required fact inputs,
review quorum modes (`one_acceptable`, `all_required`, `at_least`), optional
Human-required and Agent acceptance rules, explicit ActorRefs, and a frozen
TaskRole plus membership version. A required failed reviewer is not hidden by
another passing report.

Validation requirements pin check identity, configuration digest, Workspace,
commit, snapshot digest, and outcome. Decision requirements pin Proposal ID
and content version, Decision ID/outcome, policy reference/version/digest, and
permitted deciders. WorkUnit requirements pin exact WorkUnit version,
dependency snapshot, Execution result, and any required WorkUnitIntegration.
Every fact must belong to the Task and satisfy the Gate scope and provenance
checks.

CI PASS and Review PASS remain independent facts. A merge-readiness Gate can
require both. A TaskRole membership change advances its version and emits a
durable re-evaluation event; the frozen old evaluation becomes stale.

## Lifecycle, retry, and merge

`TaskLifecycleService` owns finite aggregate transitions, version fencing,
causal identity, idempotency, compatibility projection, and lifecycle events.
Only a current exact merge-readiness evaluation can move `active` to
`ready_to_merge`. A new current unsatisfied evaluation may revoke an older
readiness receipt. A verified retry receipt may also direct exact rework from
`ready_to_merge` to `active`; Actor transitions and arbitrary domain events
cannot clear readiness. Stale evaluations cannot perform demotion. SQLite
repeats exact-input and retry-receipt checks at the durable write boundary.

The retry policy is versioned and separate from Gate policy. V101 receipts
identify the failure kind and exact source ref, record the attempt and policy
digest, and deduplicate replay. Current budgets are 3 review request-changes,
2 validation failures, 3 Execution failures, and 1 merge failure. The same
failure replay cannot spend another attempt. While budget remains, the
`task.rework_requested` fact wakes the orchestrator; after exhaustion the
Task is blocked and V103 prevents direct repository/API re-entry to runnable
states. `transition_log` rejection counts and `max_rejections` no longer
authorize retries.

Merge flow is current satisfied merge-readiness Gate → `ready_to_merge` →
atomic TaskMerge admission → `merging` → existing MergeService/PR operation →
`done` on exact success or `blocked` on exact failure. Admission revalidates
the current evaluation and merge candidate while holding the shared
TaskIntegrationOperation/workspace lock. A stale subject cannot be silently
replaced. Workspace isolation, leases, integration serialization, and
recovery remain PR5 behavior.

## Events, concurrency, and crash recovery

`domain_event` is durable authority; EventBus is only a delivery hint. Gate
evaluation is idempotent and event-driven for ReviewReport, terminal
ValidationRun/Evidence, Decision, WorkUnit completion/integration, TaskRole
membership/version, and Gate policy revision facts.

- Concurrent evaluations of the same frozen input set deduplicate to one
  GateEvaluation. Concurrent lifecycle writers use expected Task/lifecycle
  versions; one winner writes one receipt and one lifecycle event.
- If a new fact arrives while an evaluation is being processed, the old
  evaluation remains bound to its frozen inputs. The new durable fact produces
  another evaluation; replay does not substitute it into the old evaluation.
- A crash after GateEvaluation commit but before lifecycle movement is
  recovered from that exact evaluation event. A crash after lifecycle commit
  returns the existing transition receipt and does not transition, dispatch,
  or merge twice.
- A policy revision remains historical. Its evaluation cannot authorize the
  active revision. Currentness and exact input fences run before lifecycle and
  TaskMerge admission.
- Retry receipts use unique Task/failure-kind/failure-ref identity, so event
  replay cannot consume the budget twice.
- A rework receipt can affect lifecycle only while no later lifecycle
  transition exists after its durable event sequence. Replaying the source
  failure after a later lifecycle decision cannot regress the aggregate.

## Relationship to PR5, PR6, and PR8

- **PR5:** WorkUnit DAG, isolated Workspace/Lease, completion result,
  TaskIntegrationOperation, integration serialization, and exact integration
  outcome remain authoritative.
- **PR6:** durable event claims, orchestrator wake snapshots, typed actions,
  effect guards, and receipts remain authoritative. Orchestrator directs work;
  Gate evaluates facts.
- **PR8:** Review Execution/ReviewReport and ValidationRun/Evidence remain
  authoritative. Gate reads their exact frozen identities and does not create
  a second verdict or validation result.

## Implementation checkpoints

### CHECKPOINT 3 — schema/lifecycle

V100 adds the lifecycle aggregate, immutable transition facts, audit mapping,
Gate identity and versioned policy/evaluation/input tables. V101–V107 add exact
retry receipts, TaskRole revision events, retry exhaustion fences, mutable Gate
input currentness, exact retry-rework demotion authority with replay fencing,
and Running Execution admission fencing for terminal Task lifecycle. V107
fences interactive WorkspaceLease admission/renewal against canonical
TaskRole membership. Task
lifecycle writes use optimistic Task/lifecycle versions, durable idempotency
receipts, and one-way `task.status` projection.

### CHECKPOINT 4 — Gate Engine

Policy v1 normalizes and digests bounded Review, Validation, Decision,
WorkUnit, merge-operation and lifecycle-operation requirements. Evaluations
freeze exact inputs and cannot be edited. Tests cover review quorum and Human
requirements, TaskRole membership, exact validation subject, Decision and
WorkUnit/Execution facts, plus currentness after an exact Execution result
changes.

### CHECKPOINT 5 — events/concurrency

Durable `domain_event` records drive Gate and retry consumers; EventBus remains
a hint. Source-event replay reuses its original evaluation. Duplicate
evaluations, transitions, operation admission and retry receipts are fenced by
unique identities and versions. Lifecycle and merge effect triggers recheck
the exact frozen facts at the SQLite write boundary.

### CHECKPOINT 6 — legacy authority cutover

Dispatcher readiness and all-work-completed hooks read TaskLifecycle.
Execution failure/review completion no longer recursively changes Task state;
the active completion entry point is a no-op and durable Gate/retry events
direct the next work. Legacy workflow/state and review metadata remain bounded
projection or historical storage for PR12/PR13.

### CHECKPOINT 7 — focused tests

Focused results are recorded in the table below. Recovery, dispatcher,
lifecycle, Gate, retry, merge admission, TaskIntegrationOperation, hook, API,
migration and canonical happy-path checks were run against the current branch.

### CHECKPOINT 8 — final audit

Final formatting, diff, authority grep, Git ancestry and remote branch checks
are recorded at review close. PR9 remains unmerged.

## Focused verification

The focused acceptance set covers immutable/revisioned policy and evaluation,
exact review and TaskRole membership fencing, wrong reviewer and Human
requirements, one-of/quorum/all-required behavior, exact validation subject
fields and mismatch rejection, exact Decision policy and decider, exact
WorkUnit completion/integration requirements, migration ambiguity, retry
receipt replay/exhaustion, Task lifecycle concurrency/idempotency, event
replay, merge-operation outcome fencing, REST Gate behavior, and the
canonical API happy path.

Commands and results from this review:

| Command | Result |
| --- | --- |
| `cargo fmt --all -- --check` | PASS. |
| `git diff --check` | PASS. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo check -p services --locked --offline` | PASS without warnings after removing three unused legacy mutation entrypoints. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo check -p api --locked --offline` | PASS; product crates compile without access to the crate-private WorkflowEngine mutators. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib --no-run --locked --offline` | PASS; all services unit-test callsites compile after the legacy mutation cutover. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_lifecycle::tests:: --locked --offline` | PASS, 9 tests, including concurrent version fencing, exact validation mismatch, rejection of Actor re-entry from merge-ready, and exact retry-receipt rework. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib gate_engine::tests:: --locked --offline` | PASS, 5 tests, including wrong reviewer, Human-required, N-of-M, all-required, and stale TaskRole membership. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_failure_retry::tests:: --locked --offline` | PASS, 2 tests. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib coordination_consumer::tests:: --locked --offline` | PASS, 1 test for exact lifecycle outcomes and stale-reason isolation. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib shutdown::tests:: --locked --offline` | PASS, 4 tests; shutdown selects actual running Executions rather than the lossy status projection. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_dispatcher::tests::dispatcher_retries_failed_execution_only_with_exact_durable_rework_receipt --locked --offline` | PASS, 1 test. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_dispatcher::tests::dispatcher_leaves_active_task_work_to_an_explicit_orchestrator --locked --offline` | PASS, 1 test. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_dispatcher::tests::dispatcher_keeps_permanent_executor_unavailability_blocked --locked --offline` | PASS, 1 test. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib project_hooks::tests:: --locked --offline` | PASS, 11 tests. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib recovery::tests::heartbeat_monitor_marks_stalled_executions_without_legacy_retry_authority --locked --offline` | PASS, 1 test. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib merge_service::tests:: --locked --offline` | PASS, 1 test. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_integration_operation::tests:: --locked --offline` | PASS, 6 tests, including cross-process Gate admission and operation recovery. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_service::actions::tests:: --locked --offline` | PASS, 1 test; merge lifecycle no longer advertises invalid Resume/Cancel actions. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib orchestrator_runtime::tests:: --locked --offline` | PASS, 23 tests, including TaskRole revision fencing, the Gate membership event, and the single-wake retry regression. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db --test file_backed_migration pr9_migration_maps_legacy_task_states_conservatively_and_audits_ambiguity --locked --offline` | PASS, 1 test. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db --lib interactive_workspace_lease_uses_task_role_for_insert_and_renewal --locked --offline` | PASS, 1 test; the legacy lease cannot renew or be reissued after the canonical TaskRole contradicts its singleton assignee. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_service::tests::service_tests::cases::executions::interactive_workspace_lease_uses_the_canonical_task_role_when_present --locked --offline` | PASS, 1 test; a stale singleton Agent is terminalized before adapter dispatch and receives no WorkspaceLease. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_service::tests::service_tests::cases::executions:: --locked --offline` | PASS, 54 tests, including the interactive TaskRole authority regression and the PR8 review/session execution cases. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_service::tests::service_tests::cases::claim::claim_returns_the_persisted_execution_after_dispatch_start_fails --locked --offline` | PASS, 1 test; the claim response reflects the durable failed Execution after dispatch denial. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test gate_engine_api --locked --offline` | PASS, 3 tests after aligning the JSON expectation for nullable `scope_requirement`. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test happy_path --locked --offline` | PASS, 2 tests. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib orchestrator_runtime::tests::exact_failure_has_one_wake_after_retry_lifecycle_effect --locked --offline` | PASS, 1 regression test: a failure's derived GateEvaluation and source event do not add wakes beside its exact rework receipt. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib gate_engine::tests::task_role_reviewer_snapshot_is_rechecked_against_membership_fence --locked --offline` | PASS, 1 regression test: membership insert/update events report the exact post-write TaskRole revision. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db --lib running_execution_admission_is_fenced_after_terminal_task_lifecycle --locked --offline` | PASS, 1 regression test: terminal Tasks reject both new Running Executions and completed-to-running resumption. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_service::tests::service_tests::cases::transitions::replayed_cancel_reconciles_execution_after_lifecycle_commit --locked --offline` | PASS, 1 crash-window regression: replayed cancellation stops a Running Execution after the lifecycle commit. |
| `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib task_service::tests::service_tests::cases::transitions::transition_rejects_invalid_move_and_cancel_is_idempotent --locked --offline` | PASS, 1 test; repeated cancellation remains idempotent. |
| `pnpm --dir web typecheck` | PASS earlier in PR9; no web files changed in this review. |

No workspace-wide test suite was run. These are focused crate/module checks;
they do not claim provider/live acceptance or full workspace acceptance.

## Authority audit and deferred cleanup

- **NEW AUTHORITY:** TaskLifecycleService/repository + immutable transition
  receipt; GateEngine + immutable policy/evaluation/input records; exact
  failure retry receipts.
- **AUTHORITATIVE FACTS:** ReviewReport, ValidationRun/Evidence, Decision,
  Execution, WorkUnit/WorkUnitIntegration, TaskIntegrationOperation,
  Workspace/Lease, durable DomainEvent.
- **BOUNDED COMPATIBILITY / PR12:** `task.status` projection; finite legacy
  status input mapped to aggregate lifecycle; workflow definition read/config
  views and existing response fields. WorkflowEngine mutation methods are
  crate-private; unused execution-cause, board-move, and reset mutators were
  removed, and retired recovery actions reject before reaching retained
  private recovery code. None can write lifecycle or certify a Gate.
- **HISTORICAL STORAGE / PR13:** transition log, blocked/entry-barrier/review
  columns, workflow/state configuration, GateConfig and legacy hook records.
- **PR10+:** embedded agent host and cognition/process ownership.
- **BUGS FIXED IN PR9 REVIEW:** legacy all-work-completed and executor
  terminality readers now use aggregate lifecycle; legacy recursive/retry
  paths no longer own execution rework; exact failure receipts authorize
  redispatch while permanent executor-unavailable blocks remain effective;
  stale GateEvaluations cannot revoke readiness; replayed retry receipts cannot
  regress a later lifecycle decision; coordination outcomes, agent focus and
  shutdown selection now read lifecycle or exact Execution facts. The review
  also found and fixed coupled retry-event issues: a receipt's own
  `ready -> active` transition was incorrectly treated as superseding that
  receipt; both the source failure plus its `task.rework_requested` event and
  a GateEvaluation derived from that source could independently wake the
  Orchestrator; and membership events reported a TaskRole revision one step
  ahead of the committed row. The exact receipt transition is now exempt from
  supersession, later transitions still invalidate it, and exact retry or
  exhaustion receipts suppress both their source and a GateEvaluation derived
  from it. Membership events now report the post-write revision. The audit
  also found a cancellation race where a launch could finish workspace
  preparation after the lifecycle commit, and replay of an already-cancelled
  Task action returned before reconciling a surviving Execution. V106 fences
  Running Execution insertion/resumption in SQLite; both cancellation service
  and TaskAction API replay repeat execution cleanup. A separate provenance
  check requires the exact retry
  event to carry both the persisted source `causation_id` and matching payload
  `source_event_id` before it can authorize rework. The final role-authority
  pass found that `interactive` could be interpreted as “no TaskRole” and
  inherit a contradictory singleton assignee. Service admission now resolves
  the Task's operation role before adapter dispatch, and V107 repeats active membership checks on
  WorkspaceLease insertion and renewal. Legacy singleton authority remains
  bounded to Tasks with no matching canonical TaskRole.

Physical removal of `task.status`, workflow/state tables, GateConfig,
`transition_log`, old review storage and broad API/MCP/UI alignment is deferred
to PR12/PR13 as assigned. These retained records have no contradictory
authority.

## Findings at review close

- P1: 0
- P2: 0
- P3: 0

The final authority pass found and fixed one P2: `interactive` WorkspaceLease
admission could inherit a contradictory legacy assignee despite a canonical
TaskRole. V107 and service validation now require active membership, with
insert, renewal, and service regressions. PR9 is ready for review and remains
unmerged.
