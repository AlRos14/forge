# PR13 — Destructive persistence cleanup ledger

This document records the PR13 cutovers and their focused acceptance evidence.
Architecture V2 remains the contract for the surrounding plan. PR13 does not
change PR14 branding or introduce PR15 scenarios.

## Baseline and Git

- Repository: `AlRos14/forge`
- Base branch and merge-base at review start: `origin/main` /
  `3d3b6b196672568c85e4c3cd2c11f010b1f62d9a`
- PR12 merged HEAD: `365c4b938a4b54f523c02bd7775a80850a8574a0`
- PR13 branch: `feat/plan-pr13-destructive-persistence-cleanup`
- Initial PR13 review HEAD: `07261288f339d59875f918680511e8bfc2148015`
- Initial schema head: V120; PR13 schema head: V132.
- Corrected implementation HEAD: `034111177c4e8d15858dcabc85193b5c65d75c24`
  (`fix(pr13): close recovery and migration audit gaps`).
- Final PR13 HEAD: the branch tip after this ledger commit. Publication records
  its exact SHA and verifies that `origin/feat/plan-pr13-destructive-persistence-cleanup`
  matches it.

## Cutover matrix

| Family and source objects | Current target and preserved history | Migration and code cutover | Acceptance evidence |
|---|---|---|---|
| Singular role assignment: `task_role_assignment`, `task.assignee_type`, `task.assignee_id` | `TaskRole` and `RoleMembership` remain authoritative. Exact residual projection rows are in `task_role_assignment_history` and `pr13_task_assignment_projection_history`. | V127 archives/drops the singleton table; V128 archives values and rebuilds `task`. Role adapters retain target membership persistence and remove singular fallbacks/projections. | `unified_assignee_migration` and focused RoleMembership/Execution tests; tests run listed below. |
| Execution session projection: `execution.agent_session_id` | Exact `HarnessSession` remains the only current Execution/session binding. Ambiguous scalar values are retained in `pr13_execution_session_projection_history`. Native session and encrypted child rows remain under `agent_session_history` and protected child tables. | V124 archives then drops the scalar; V126 renames native session storage; V132 detaches old cognition scope FKs while preserving the historical session rows. Runtime writers bind exact HarnessSession IDs only. | `pr13_execution_session`; `pr13_agent_session_history`; `pr13_execution_session` runtime tests where applicable. |
| V081 task planning: `task_plan_revision`, `task_plan_approval`, `review_evidence_bundle`, `task.plan` | Exact old values live in `pr13_legacy_record_history`. V098-created Artifacts remain only where the source Execution/output was proven. `legacy_task_plan_artifact_migration` retains its receipt rows and canonical Artifact FK without the retired source FK. | V125 copies source rows and detaches the V098 ledger before dropping V081 tables/column. Planner-only retry/prompt runtime is removed. | `pr13_task_plan_history`; migration receipt counts and raw JSON assertions. |
| Task state and workflow: `task.status`, `task_state_config`, `entry_barrier_json`, `project.workflow_definition`, transition logs, deferred dispatch, Task move operations | `TaskLifecycle`, Gates, current board ordering and durable receipts remain target authority. Exact retired values and transition/move provenance remain in `pr13_legacy_record_history`; V100 lifecycle audit remains. | V129 archives fields and rebuilds `task`; V130 archives workflow/default-role/deferred-dispatch/move rows and drops obsolete columns/tables. `TaskRepo::create[_in_tx]` inserts Task and TaskLifecycle in one transaction; the retired status projection trigger is not restored. | Normal and subtask Task creation, injected rollback, missing-lifecycle cutover rejection, restart, and lifecycle migration tests. |
| Legacy Review runtime/storage: `review`, ReviewRunner audit/follow-up runtime, V099 migration ledgers | Exact remaining `review` rows are in `pr13_legacy_record_history`. V099 provenance ledgers remain detached from the deleted `review` source; generic ReviewReport Artifacts, ValidationRuns and Evidence remain target history. | V131 copies rows, rebuilds/detaches the two V099 receipt tables and drops `review`. The old `crates/review` package was removed after confirmation that it had no external consumers. | `pr13_review_cleanup`; current ReviewReport/ValidationRun focused coverage. |
| Main/Project Agent, Chat/Room, Genesis, memory and Attention tables | Every listed PR11 row is stored with source table, source identity and exact column payload in `pr13_legacy_record_history`. Built-in V076 skills are archived as their actual two Skill and two revision records. `user`, `project`, `agent_identity` and `agent_profile` remain target storage. | V132 archives and removes retired runtime tables, old Project columns and their triggers/indexes/FKs. No canonical Actor, Artifact, Execution or Evidence is inferred from legacy rows. | `pr13_archives_legacy_agent_context_before_dropping_tables`; fresh schema/archive assertion; FK and integrity checks. |
| Project OS: Charter, documents, decisions, baselines, milestones, readiness, governance, Project media attachments and old primary pointers | Release-required values are first copied into immutable per-Release history in V121. Other legacy rows remain in `pr13_legacy_record_history`; DomainEvents, migration ledgers, generic media and target receipts remain. | V121 snapshots Release dependencies; V122 detaches Release and tombstone FKs from Project OS pins/references; V132 archives and drops the remaining Project OS schema. | Authentic V120 Release upgrade tests, public GET/list equivalence, digest and FK/integrity gates. |
| ProjectRelease historical contract | `project_release` keeps release identity, revision/sequence, references, authorization provenance and `snapshot_digest`. `project_release_history_{milestone,definition,readiness,charter,reference,media_pin}` stores its exact historical inputs and pins. Media assets/tombstones remain target history. | V121 copies exact stored values after completeness assertions; V122 removes Project OS FKs and old release reference/pin tables. `release_from_record` reads only the detached representation and verifies `release_snapshot_digest`; it never reconstructs from present-day Project OS state. | `project_release_history` DB test and `project_release_history` API test: authentic V120 snapshots, GET and milestone list, exact identities/JSON/digest, shared assets, tombstones, corrupt/incomplete fail-closed cases, repeated migration, FK/integrity. |
| Integration default implementer | Exact `default_implementer` ActorRef stores Agent or Human identity; NULL means no default. Invalid/unresolved source pairs remain in `pr13_integration_default_implementer_issue` and the old pair is archived in `pr13_project_integration_legacy_defaults`. `token_secret_ref` remains active. | V123 deterministically maps only exact valid Actor identities, rebuilds `project_integration`, and removes the legacy pair. | `pr13_integration_default` covers Agent, Human, NULL and invalid/unresolved values. |
| One-Task/one-Workspace assumptions in recovery, resume, follow-up, merge and cleanup | Recovery and execution target exact `WorkUnit`, `Execution`, `Workspace` and lease identities. The Task integration Workspace remains a distinct, unique, serialized integration target defined by the architecture. | Runtime fallback from Task to an Execution or Workspace selected by recency is removed. Merge requires a WorkUnit and one Gate-pinned integration whose Workspace and commit equal the current integration target. Recovery admission rechecks exact identity in its durable transaction. | Recovery with sibling WorkUnits/Workspaces rejects the sibling Workspace; admission race tests cover stale WorkUnit version, membership loss, Workspace failure, duplicate calls and external provider failure. Existing WorkUnit cleanup coverage verifies sibling branches/worktrees remain untouched. |

## Migration sequence

| Version | Durable effect |
|---|---|
| V121 | Copies complete immutable Release snapshots, references and media pins; rejects incomplete source graphs before cutover. |
| V122 | Rebuilds Release and media tombstone FKs around detached history; removes Project OS release references/pins and checks FK/integrity state. |
| V123 | Stores exact integration default ActorRefs and archives invalid or original legacy defaults. |
| V124 | Archives every non-NULL `execution.agent_session_id` projection, then drops the column. |
| V125 | Archives V081 plan/review-bundle rows and detaches V098 provenance from deleted source rows while retaining receipts. |
| V126 | Renames native AgentSession storage to `agent_session_history`; encrypted/protected children continue to reference the preserved history. |
| V127 | Archives and drops `task_role_assignment`; target membership storage and exact actor-delete cleanup remain. |
| V128 | Archives singleton Task assignment values and rebuilds Task without assignee columns. |
| V129 | Verifies every Task has TaskLifecycle, archives old Task status/configuration, rebuilds Task and removes status projection triggers. |
| V130 | Archives workflow, transition-log, deferred-dispatch, default-role-setting and Task-move history; drops retired columns/tables. |
| V131 | Archives old Review rows; detaches and preserves V099 migration ledgers; drops the Review table. |
| V132 | Archives PR11 legacy rows and Project OS fields, checks expected row counts/FKs/integrity, then removes retired tables and authority triggers. |
| V133 | Adds immutable follow-up operation receipts so one idempotency key replays its exact child Execution and cannot authorize different request content. |

V001–V120 are unchanged. SQLite rebuild migrations use explicit direct-connection
stages where PRAGMAs require it. Their migration receipt is written inside the
same durable transaction as the schema change. On a failed direct migration the
runner rolls back and restores `foreign_keys=ON` and `legacy_alter_table=OFF` on
the pooled connection. A restart skips committed receipts. V120→V132 restart
and V132→V133 restart were verified separately.

## Preserved invariants and data locations

- `TaskLifecycle` is the only persisted Task state authority. Canonical Task and
  subtask creation inserts Task plus TaskLifecycle in the same transaction;
  a failure between inserts rolls back both. A missing historical lifecycle
  aborts V129.
- `TaskRole` and `RoleMembership` remain target storage. PR13 does not synthesize
  membership from singleton assignments.
- `Execution`, `HarnessSession`, `WorkUnit`, `Workspace` and `WorkspaceLease`
  retain exact identities. Ambiguous `agent_session_id` values are history,
  never HarnessSession creation inputs.
- Recovery admission uses `BEGIN IMMEDIATE` to check the blocked parent,
  Task/version/annotations, lifecycle, exact WorkUnit version/assignment and
  membership, Workspace scope/status, ArtifactInputs and lease before committing
  lifecycle transition, annotation clearing, child Execution and receipts/events
  together. Provider dispatch starts only after that local commit.
- A provider-start failure after admission preserves the one exact child and
  lineage, records the failure/block, and cannot replay the stale parent as a
  second child.
- ProjectRelease snapshots remain immutable and self-contained for public GET.
  Invalid digest or incomplete history fails closed. No current milestone,
  Charter, readiness or baseline state is consulted to fill historical gaps.
- DomainEvents, idempotency receipts and migration ledgers remain history.
  V088, V089, V098, V099, V100 and V118 provenance is retained; V098/V099 source
  FKs are detached only where their source table is removed.
- Follow-up receipts bind a stable request key and content digest to the exact
  parent and admitted child. A replay returns that Execution without replaying
  DomainEvents or provider start. Project teardown removes these immutable
  receipts only under its existing guarded teardown transaction.
- PR11 writers and operational readers do not return. `agent_session_history`
  remains only for explicitly retained credential/history reads; no Project OS
  writer, Release creator or readiness evaluator is restored.
- `pr13_legacy_record_history` stores source table, source identity and the
  captured original column values in JSON text. The original JSON strings are
  values inside that JSON record; migration-only insert plus immutable update
  and delete triggers protect the archive after each cutover.

## Recovery and exact workspace acceptance

The former ordering that activated a Task and cleared its blocking annotation
before admitting its child Execution is removed for recovery paths. Durable
admission commits the lifecycle transition, annotation clear, child Execution,
ArtifactInput bindings, optional exact WorkspaceLease and both DomainEvents in
one SQLite transaction. Failed admission leaves the prior blocked state and
annotation intact. The provider is called only after commit; its failure is a
new recorded attempt and does not create another child on retry.

Recovery, follow-up and re-execution obtain Workspace by the parent Execution's
exact `workspace_id`, then verify that `workspace_scope.work_unit_id` matches
the same parent `work_unit_id`. No Task-scoped or temporally selected Workspace
is substituted. The integration Workspace used for Task-level merge, diff and
terminal operations remains the unique serialized integration target.

## Removed test coverage review

Tests removed with workflow, Task.status, special Review, Main/Project Agent,
Room/Chat, memory, Attention and Project OS code exercised behavior whose
runtime authority is retired. Coverage that also protected current invariants
was retained or moved to focused lifecycle, TaskRole/RoleMembership,
WorkUnit/WorkspaceLease, HarnessSession, media, migration and Release tests.
The replacement gates include Task creation rollback, lifecycle cutover
rejection/restart, exact recovery lineage and race tests, exact PR13 family
archives, public Release equivalence, FK checks and cleanup that preserves
sibling WorkUnit branches. No retired suite is reintroduced wholesale.

## Test and CI evidence

Results recorded from this implementation session; commands use cached
`target/` and `--locked --offline` where Cargo applies:

| Evidence | Result |
|---|---|
| `cargo test -p api --test project_release_history --locked --offline` | PASS, 2 tests; includes authentic V120→V132 public GET/list equivalence and adversarial historical cases. |
| `cargo test -p db --test pr13_release_history v121_copies_exact_release_snapshot_and_v122_detaches_project_os_foreign_keys --locked --offline` | PASS, 1 test. |
| `cargo test -p db --test pr13_execution_session --locked --offline` | PASS, 1 test; ambiguous session projection is preserved verbatim without creating HarnessSession. |
| `cargo test -p db --test pr13_agent_session_history --locked --offline` | PASS, 1 test; native/protected session history remains. |
| `cargo test -p db --test pr13_integration_default --locked --offline` | PASS, 1 test. |
| `cargo test -p db --test pr13_task_plan_history --locked --offline` | PASS, 1 test; V081/V098 provenance remains exact. |
| `cargo test -p db --test pr13_review_cleanup --locked --offline` | PASS, 1 test; Review rows and V099 receipts survive source-table retirement. |
| `cargo test -p db --test file_backed_migration pr13_fresh_database_drops_legacy_storage_and_keeps_exact_builtin_skill_archive --locked --offline` | PASS, 1 test; fresh schema/archive plus no remaining trigger, index or view references to retired tables/columns. |
| `cargo test -p db --test file_backed_migration pr13_v120_empty_database_upgrades_and_restarts_cleanly --locked --offline` | PASS, 1 test; V120 empty database to V132, second pass, expected PR13 receipts, PRAGMAs, FK and integrity. |
| `cargo test -p db --test unified_assignee_migration --locked --offline` | PASS, 3 tests; exact singleton archive, empty upgrade and multi-Actor RoleMembership remain authoritative. |
| `cargo test -p db --test pr5_workunit integration_lookup_stays_exact_with_multiple_work_unit_workspaces --locked --offline` | PASS, 1 test; integration lookup remains exact with sibling WorkUnit Workspaces. |
| `cargo test -p db --test file_backed_migration pr13_lifecycle_cutover_accepts_restart_with_no_new_receipts_or_rows --locked --offline` | PASS, 1 test; second migration pass, PRAGMAs, FK and integrity checks. |
| `cargo test -p db --test file_backed_migration pr13_v129_rejects_and_rolls_back_a_historical_task_without_lifecycle --locked --offline` | PASS, 1 test. |
| `cargo test -p db --lib failed_direct_migration_restores_pragmas_and_rolls_back_schema_and_receipt --locked --offline` | PASS, 1 test. |
| `cargo test -p db --lib migration_creates_schema_and_enforces_foreign_keys --locked --offline` | PASS, 1 test; normal Task+TaskLifecycle insert and injected failure between inserts rolls both rows back. |
| `cargo test -p db --lib test_normalize_failure_kinds_migration_backfill --locked --offline` | PASS after repairing the test's V055 fixture to use columns that exist in that historical schema. |
| `cargo test -p services --lib crash_recovery_ --locked --offline -- --test-threads=1` | PASS, 10 tests after updating obsolete test fixtures to canonical TaskRole and TaskLifecycle receipts. |
| Focused recovery admission cases: artifact binding rollback, WorkUnit version race, membership loss, unusable Workspace, concurrent duplicate admission and provider failure after durable admission | PASS in the filtered recovery run; rejected paths preserve TaskLifecycle, annotations, WorkUnit, parent lineage, Workspace and receipts. |
| `cargo test -p services --lib parent_mutations_keep_parent_and_subtask_order_coherent --locked --offline` | PASS, 1 test. |
| `cargo test -p services --lib recovery_uses_the_parent_work_units_workspace_when_task_has_siblings --locked --offline` | PASS, 1 test; exact parent Workspace accepted, sibling Workspace rejected. |
| `cargo test -p services --lib integration_pins_success_and_aborts_conflict_without_losing_workunit_workspace --locked --offline` | PASS, 1 test; cleanup preserves the WorkUnit branch/workspace. The fixture now requests `Active` only when lifecycle is not already `Active`. |
| `cargo test -p services --lib merge_service::tests:: --locked --offline` | PASS, 2 tests; legacy entry requires exact Gate evaluation and Task without WorkUnits cannot infer a merge source by recency. |
| GitHub Actions for `feat/plan-pr13-destructive-persistence-cleanup` | NOT RUN: the repository lists active CI, CodeQL, Release and OpenSSF Scorecard workflows, but no run entries were returned for the PR13 branch. No remote CI pass is claimed. |

One earlier broad recovery filter returned 13 passes and 6 failures in
pre-existing legacy test fixtures (five used the retired `coder` alias and one
changed lifecycle state without its required receipt). Those fixtures were
updated; the focused crash-recovery rerun passed. This is retained here so the
intermediate run is not misreported as green.

The first supplementary WorkUnit cleanup run exposed an already-Active Task
after prior integration steps; the fixture attempted an `Active` to `Active`
transition, which the lifecycle transition graph rejects. The fixture now
checks the persisted lifecycle first and only transitions when necessary. The
same focused test then passed; production lifecycle behavior was not changed.
A preceding `--nocapture` diagnostic attempt briefly failed to compile because
temporary state-printing instrumentation referenced an unqualified repository
type; the diagnostic was corrected for inspection and removed before the final
test run.

The first run of the empty V120 upgrade test also failed on its test assertion:
it assumed receipt count equaled the maximum migration version. The V120
history has 119 receipts with head V120, so the corrected test checks the head
and receipt delta separately; the rerun passed.

The first retained-schema dependency assertion flagged `archived_at` on the
still-required `agent_identity` table, not on Task. The check was narrowed to
Task-specific retired columns; the rerun passed with no remaining trigger,
index or view references to retired names.

Deliberately not run: `cargo test --workspace`, all-crate DB/API suites,
`cargo build --workspace`, `cargo clippy --workspace`, and `cargo clean`.
Focused tests cover the reviewed cutovers; workspace-wide results are not
claimed. No PR currently exists for the branch, and no GitHub Actions run was
available for it at publication.

## Remaining bounded risks

- The migration-only legacy archive is not a new query/API surface; consumers
  must not reinterpret ambiguous archived rows as current authority.
- Release GET deliberately retains a narrow historical read path. It cannot
  create Releases or recalculate readiness, and fails closed when digest or
  copied source identity checks do not hold.
- Provider execution remains an external effect after durable local admission;
  an uncertain provider result is represented by the exact admitted Execution
  and requires ordinary recovery, never an inferred duplicate.
- No workspace-wide Cargo build/test/clippy or remote CI green claim is part of
  this evidence set.

## Review census

After the recovery ordering, historical Release equivalence, TaskLifecycle
creation contract, and Task-to-Workspace recency fallbacks are closed:

```text
P0 = 0
P1 = 0
P2 = 0
P3 = 0
BUG_STILL_LIVE = 0
```

PR13 is ready for independent review after the final branch push and remote ref
verification. This ledger does not authorize merge.

## PR publication

No PR exists for this branch as of publication, so no PR body was changed and
none was opened. Create it from:

`https://github.com/AlRos14/forge/compare/main...feat/plan-pr13-destructive-persistence-cleanup?expand=1`

## PR13 microfix audit — 2026-10-13

This update is scoped to the two lifecycle/admission P1 findings. It preserves
the prior PR13 record above and does not start PR14.

### Git checkpoint

- Branch: `feat/plan-pr13-destructive-persistence-cleanup`.
- Base and merge-base: `origin/main` / `3d3b6b196672568c85e4c3cd2c11f010b1f62d9a`.
- Microfix initial HEAD: `534e82d1f111890cb3bbadfad1848aae1988d9d6`.
- Completion report records the microfix commit SHA and remote read-back.

### Findings and fixes

**P1-1 — TaskLifecycle activation reused an entity-wide idempotency key.**
`execution-start:{task_id}:active` and the Backlog key did not distinguish a
later valid transition from an earlier receipt. Activation keys now include the
exact source TaskLifecycle version; the Backlog-to-Ready and Ready-to-Active
edges use their respective source versions. `TaskLifecycleService` builds the
same receipt-bearing transition records consumed by the PR9 repository path;
WorkUnit admission writes them through the existing CAS and content comparison
in the same transaction as Execution admission. Retries of one transition keep
the same key and receipt, a later lifecycle version gets a new key, and reuse
for different content still conflicts. No historical receipt is rewritten.

**P1-2 — ordinary launch and follow-up could persist lifecycle or clear
recovery metadata before child admission.** The former shared activation helper
is removed. Initial launch, ordinary re-execute, and follow-up now prepare their
lifecycle plan, frozen executor snapshot, and ArtifactInputs before writes;
Task/version, lifecycle receipt(s), optional metadata cleanup, exact Execution,
ArtifactInputs, WorkspaceLease, and durable events commit together through the
bounded WorkUnit admission primitive. Recovery continues through its existing
specialized atomic primitive.

| Path | Preflight | Durable effects before admission | One local commit | External boundary |
|---|---|---|---|---|
| Initial WorkUnit launch | Actor, RoleMembership, exact WorkUnit/version, capacity, Task lifecycle, exact ready Workspace, snapshot and ArtifactInputs | None | Lifecycle activation when required, Execution, ArtifactInputs, lease and events | Provider start follows commit |
| Follow-up | Exact terminal parent, exact WorkUnit/Workspace and assignment, reusable exact HarnessSession, snapshot and inherited ArtifactInputs | None; blocking cleanup is only a plan | Lifecycle activation, metadata cleanup, child lineage, ArtifactInputs, lease, events and V133 request receipt | Provider start follows commit; a repeated key returns the admitted child without starting another provider attempt |
| Ordinary re-execute | Exact parent, WorkUnit/Workspace, Actor/membership, snapshot and ArtifactInputs | None | Lifecycle activation when required, child lineage, Execution, ArtifactInputs, lease and events | Provider start follows commit |
| Explicit recovery | Existing exact recovery preflight | None | Existing recovery transaction commits Blocked→Active, annotation cleanup, child, ArtifactInputs, lease and events | Provider start follows commit |

The V133 receipt stores a digest of the stable request fields, exact parent, and
child ID. Same-key/same-content retries return the recorded child, including
after an uncertain response. Same-key/different-content attempts return
`IdempotencyConflict`. A new user-authorized follow-up uses a new key. REST and
MCP require the key; the web client retains it for a retry of unchanged form
content and clears it after success. No server-generated random ID is used to
turn a retry into a new operation.

### Focused evidence

| Command or check | Result |
|---|---|
| `cargo test -p services --lib execution_activation --locked --offline -- --test-threads=1` | PASS, 2 lifecycle key/replay tests; verifies versions, receipts, events and conflict on changed content. |
| `cargo test -p services --lib ordinary_launch_ --locked --offline -- --test-threads=1` | PASS, 3 tests for snapshot, artifact binding, and WorkUnit-version rejection. |
| `cargo test -p services --lib ordinary_repository_launch_workspace_readiness_race_rolls_back_lifecycle --locked --offline -- --test-threads=1` | PASS, Workspace readiness race leaves no activation. |
| `cargo test -p services --lib ordinary_provider_start_failure_keeps_one_admitted_execution_and_event --locked --offline -- --test-threads=1` | PASS, post-commit provider failure leaves one admitted Execution and one start event. |
| `cargo test -p services --lib ordinary_reexecute_ --locked --offline -- --test-threads=1` | PASS, 2 snapshot/version rejection tests. |
| `cargo test -p services --lib follow_up_retry_replays_exact_child_and_new_key_authorizes_next_child --locked --offline -- --test-threads=1` | PASS; exact retry replay, changed-content conflict, and new-key authorization after the first child is terminal. |
| `cargo test -p services --lib concurrent_follow_up_retries_replay_one_child_and_one_activation_receipt --locked --offline -- --test-threads=1` | PASS; competing same-key requests return one child with one lifecycle receipt and no duplicate events. |
| Follow-up rejection tests for ended HarnessSession, RoleMembership change, and ArtifactInput pin failure | PASS, 3 focused tests; TaskLifecycle and recovery annotations remain intact. |
| `cargo test -p services --lib recovery_admission_ --locked --offline -- --test-threads=1` | PASS, 3 existing recovery transaction race/rollback tests. |
| `cargo test -p services --lib follow_up_ --locked --offline -- --test-threads=1` | PASS, 17 tests; canonical exact WorkUnit/HarnessSession fixtures, independent sibling WorkUnit progress, idempotent replay, membership/session/artifact rejection, and metadata preservation. |
| `cargo test -p services --lib follow_up_work_unit_version_change_rolls_back_lifecycle_and_metadata --locked --offline -- --test-threads=1` | PASS; deterministic WorkUnit revision change during admission rolls back lifecycle, recovery metadata, child, event, and follow-up receipt. |
| `cargo test -p services --lib rejected_recovery_cancel_preserves_blocking_metadata --locked --offline -- --test-threads=1` | PASS; rejected cancellation preserves annotation; accepted cancellation clears it only after lifecycle success. |
| `cargo test -p db --test file_backed_migration v133_follow_up_receipts_upgrade_from_v132_and_restart_cleanly --locked --offline` | PASS, V132→V133 upgrade/restart, foreign-key check, and integrity check. An initial attempt exposed an unnecessary nested transaction; V133 now relies on the migration runner's transaction. |
| `cargo test -p api-types --lib follow_up_requires_a_stable_idempotency_key --locked --offline` | PASS; request decoding requires the key. |
| `cargo test -p mcp-server --lib tools_list_returns_descriptors --locked --offline` | PASS; MCP advertises the required key. |
| `cargo test -p api --test pr12_public_surface task_and_execution_routes_enforce_project_visibility --locked --offline -- --test-threads=1` | PASS, public route contract and authorization. |
| `pnpm typecheck` | PASS. |
| `cargo check -p services -p db -p api -p mcp-server --locked --offline` | PASS. |
| `cargo fmt --all -- --check`; `git diff --check` | PASS after final edits. |

The first expanded `follow_up_` filter exposed eight stale fixtures or
assertions: they constructed Task-scoped Executions without the exact
WorkUnit/HarnessSession required by the existing validation, asserted the
retired Task-wide running-role restriction, or expected an old error message.
The fixtures now use canonical WorkUnit admission and exact HarnessSession
records; the retired restriction test now verifies that a child remains bound
to its parent WorkUnit while an independent sibling WorkUnit runs. The final
same filter passed all 17 tests. A subsequent deterministic follow-up
WorkUnit-version race test also passed and verified no receipt survives the
rollback. No production authority guard was relaxed.
No workspace-wide test/build or global clippy run was made, per the task
constraint. Remote CI was not run.

### Residual risks and final census

- Provider execution remains outside SQLite. If provider start fails after
  admission, the exact Execution and receipt remain durable; retrying the same
  follow-up key cannot make a duplicate child or provider start. Provider retry
  remains an exact Execution/recovery operation.
- REST/MCP clients must preserve one key across a transport retry and use a new
  key for a separately authorized follow-up.
- The static call-site audit found no remaining pre-admission lifecycle
  activation in initial launch, follow-up, or ordinary re-execute; their
  lifecycle records are part of WorkUnit admission. Blocking metadata is only
  cleared through that admission plan or after accepted cancellation. The old
  entity-wide activation keys are absent, and no legacy Task status, singleton
  assignee, or recency-based WorkUnit/Workspace/HarnessSession selection was
  introduced. PR5/PR6/PR9 Actor, RoleMembership, WorkUnit, capacity, Workspace,
  and receipt guards remain in force.
- No P0/P1/P2/P3 finding remains in the audited scope.

```text
P0 = 0
P1 = 0
P2 = 0
P3 = 0
BUG_STILL_LIVE = 0
```

Final HEAD, push SHA, remote-ref equality, clean-tree check, and final READY
decision are recorded in the completion report. Merge remains unauthorized.
