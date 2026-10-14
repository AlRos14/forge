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

## PR13 microfix audit — 2026-10-14

This update closes the follow-up block-ownership finding and the remaining
Blocked-to-Active metadata gap. It leaves V121–V133 and the earlier PR13
implementation intact; no migration or PR14 work was added.

### Findings and decisions

**P1 — Follow-up could clear a sibling WorkUnit's current block.**
`TaskLifecycle` is the Task-wide aggregate block. Its current `reason_ref` and
the exact transition receipt at the current lifecycle version identify the
blocking Execution when the block is Execution-owned. The source Execution
must belong to the same Task and exact open WorkUnit as the new child. Each
present `error_annotation` and `blocked_json` must separately identify that
Execution; they may carry different information and are not required to match
each other. An admission for a sibling WorkUnit is rejected while a different
Execution owns the current block. Its TaskLifecycle and recovery evidence stay
unchanged and usable.

The ordinary WorkUnit admission transaction now rechecks the exact current
lifecycle receipt, source Execution, WorkUnit, parent lineage, each metadata
identity, Task version, metadata snapshots, and absence of Task-level
`failed_json`. The existing Task/WorkUnit CAS remains. Initial launch has no
parent identity and cannot resolve a blocked Task. Follow-up and ordinary
re-execute may resolve a block only when their exact parent is that block's
Execution on the same WorkUnit. Cleanup and lifecycle activation remain in the
same commit as child Execution, ArtifactInputs, WorkspaceLease, receipts, and
events.

The failure and manual-stop writers now leave an existing sibling block's
annotation untouched when a second WorkUnit fails or stops. Startup recovery
also confirms its lifecycle block result before writing its annotation.
Explicit recovery now checks the same current lifecycle and per-field identity
under its existing transaction. A generic manual Blocked-to-Ready/Active
transition is rejected while Task blocking or failure metadata is pending;
accepted cancellation remains a Task-wide resolution. Task-level failure and
restart paths fail closed when separate Execution block metadata exists.

**P2 — Blocked activation could leave misleading metadata.**
The former follow-up guard depended only on `error_annotation`, and ordinary
initial launch/re-execute could activate without a cleanup plan. Exact
`blocked_json`-only metadata is now cleared atomically; both present fields
must resolve independently to the same exact block owner. An Execution block
without parseable, matching evidence is preserved and cannot be activated
through an unrelated launch. `failed_json` remains Task-level failure history;
ordinary admission and explicit Execution recovery do not erase it. Task
unblocked delivery now covers annotation-only as well as `blocked_json`-only
resolutions.

### Provider boundary

Provider start stays outside SQLite. A replay of a V133 same-key/same-content
follow-up returns its durable child with `idempotent_replay`; the REST dispatch
path does not start that child again. A committed Running Execution left by a
process crash is covered by startup recovery: exact resumable sessions enter
the existing automatic recovery candidate path, while an exact non-resumable
WorkUnit Execution receives a manual recovery annotation and can be retried
through exact Execution recovery. A live process that leaves an attempt idle
is covered by stalled-Execution reconciliation and the same exact recovery
route. No extra dispatch store or provider transaction was introduced.

### Focused evidence

| Command or check | Result |
|---|---|
| `cargo test -p services --lib follow_up_ --locked --offline -- --test-threads=1` | PASS, 20 tests, including sibling-block preservation, exact-parent replay/concurrency, `blocked_json`-only cleanup, ArtifactInput rollback, WorkUnit version race, and same-WorkUnit sibling progress. |
| `cargo test -p services --lib execution_activation_ --locked --offline -- --test-threads=1` | PASS, 2 tests; Backlog keys use exact source versions and repeated legal activation cycles retain stable retries with distinct receipts. |
| `cargo test -p services --lib durable_admission_rejects_block_owner_changed_after_preflight --locked --offline -- --test-threads=1` | PASS; durable admission rejects stale A ownership using current CAS versions after lifecycle now points to B. |
| `cargo test -p services --lib ordinary_reexecute_resolves_only_its_exact_current_block --locked --offline -- --test-threads=1` | PASS; exact parent re-execute commits one activation, cleanup, child and start event. |
| `cargo test -p services --lib initial_launch_on_sibling_work_unit_preserves_current_block --locked --offline -- --test-threads=1` | PASS; a parentless launch cannot clear another WorkUnit's current block. |
| `cargo test -p services --lib recovery_admission --locked --offline -- --test-threads=1` | PASS, 4 tests; concurrent single-child admission and ArtifactInput, RoleMembership, and WorkUnit-version rollback checks. |
| `cargo test -p services --lib secondary_work_unit_failure_does_not_replace_current_block_metadata --locked --offline -- --test-threads=1` | PASS; a second failed WorkUnit leaves the current block owner and annotation unchanged. |
| `cargo test -p services --lib manual_lifecycle_unblock_cannot_leave_execution_metadata_stale --locked --offline -- --test-threads=1` | PASS; manual lifecycle transition preserves evidence and rejects the state change. |
| `cargo test -p services --lib task_failure_metadata_cannot_replace_an_exact_execution_block --locked --offline -- --test-threads=1` | PASS; Task-level failure metadata cannot replace the exact Execution block. |
| `cargo test -p services --lib ordinary_provider_start_failure_keeps_one_admitted_execution_and_event --locked --offline -- --test-threads=1` | PASS; provider failure after commit leaves one durable admitted Execution and start event. |
| `cargo test -p services --lib failed_external_start_after_durable_recovery_does_not_replay_old_attempt --locked --offline -- --test-threads=1` | PASS; failed external recovery start does not replay the old attempt. |
| `cargo test -p services --lib crash_recovery_cancels_running_executions --locked --offline -- --test-threads=1` | PASS; startup recovery handles committed Running work. |
| `cargo fmt --all -- --check`; `git diff --check` | PASS after final edits. |

The unmodified V121–V132 migration contracts and API/MCP request schemas were
not rerun. No workspace-wide tests/build, global clippy, or remote CI run was
made. Existing PR13 Actor, RoleMembership, WorkUnit, capacity, Workspace,
ArtifactInput, lease, recovery, event, and immutable-receipt guards remain in
force.

The existing ordinary admission failure suites were also rerun after this
microfix: `cargo test -p services --lib ordinary_launch_ --locked --offline --
--test-threads=1` passed all 3 tests, and
`cargo test -p services --lib ordinary_reexecute_ --locked --offline --
--test-threads=1` passed all 3 tests. These cover snapshot failure, artifact
rejection, WorkUnit-version rollback, and exact blocked-parent resolution.
Provider failure after durable admission is covered by the separate focused
test listed above.

### Residual risks and final census

- If provider start is accepted externally but its response is lost, the
  durable Execution may require the existing crash/stall reconciliation and
  exact recovery route. Same-key follow-up replay cannot issue a second start.
- A failed Execution on a sibling WorkUnit remains an independent open-scope
  attempt; it cannot replace the current Task-wide block. After that block is
  resolved, the sibling may be explicitly re-executed by its own identity.
- No P0/P1/P2/P3 finding remains in the audited scope.

```text
P0 = 0
P1 = 0
P2 = 0
P3 = 0
BUG_STILL_LIVE = 0
```

The completion report records final Git SHAs, remote read-back, clean-tree
verification, and the independent-review status. Merge remains unauthorized.

## PR13 closure supplement — multi-WorkUnit crash recovery — 2026-10-14

### Cause and reproduction

`recover_executions()` assigned the automatic-resume policy to a Task's full
set of stopped Executions only when every member had an exact resumable
HarnessSession. With two exact WorkUnits but no resumable sessions it wrote
`recovery_target_ambiguous`, no `blocked_execution_id`, and no recovery action.
The aggregate TaskLifecycle then became Blocked with a Task-version
`reason_ref`. Existing Task recovery required one blocked Execution; ordinary
re-execute, follow-up, initial launch, and manual lifecycle transitions could
not select a member of this set. The pre-fix service test exercised CrashRecovery,
Execution cancellation, WorkUnit lineage, lease revocation, TaskLifecycle and
annotations, then confirmed that both exact parents and Task-level recovery
were rejected without creating a child.

A second reachable boundary was confirmed with a SQLite failure trigger:
CrashRecovery commits the Blocked lifecycle before its separate annotation
write. If that latter write is interrupted, durable Execution and WorkUnit
identities survive with an ambiguous lifecycle but no annotation. The former
recovery surface could not resolve this state either.

### Alternatives and decision

- **A — resolve the whole ambiguous block:** rejected as unnecessary. It would
  require describing and deciding all affected Executions together.
- **B — select one exact target:** chosen. A Human request binds Task, parent
  Execution, WorkUnit, Task version, lifecycle version, WorkUnit version, and
  idempotency key. In a Blocked ambiguous Task the first exact choice atomically
  activates the aggregate Task and admits that child; remaining Manual
  Executions remain individually selectable while Active.
- **C — reuse an existing transition or recovery receipt:** unavailable.
  Existing recovery receipts and routes require an exact owner and cannot
  express a human selection from a block with no single Execution owner.

The new REST request is the minimum public contract because the previous
surface had no operator route for selecting a WorkUnit Execution. It reuses
TaskLifecycle, WorkUnit, Execution, WorkspaceLease, and DomainEvent; no new
table, scheduler, or state machine was added.

### Correction

Automatic recovery is now decided per exact Execution. A resumable candidate
can enter Auto while a sibling remains Manual; eligible Auto candidates do not
create an aggregate block when the Task is Active and has no pending blocking
or failure metadata. An Auto parent stops being a candidate after a durable
child exists, preventing repeated dispatch after a lost provider response or
process restart.

The new `POST /tasks/{task_id}/executions/{execution_id}/recovery` route admits
one explicitly selected cancelled CrashRecovery, AgentTimeout, or
GracefulShutdown parent. For an ambiguous Blocked Task, the candidate must be
Manual; while Active, a Human may explicitly authorize a fresh child from an
exact Manual or Auto parent. Its receipt records the requesting Human, exact
lineage, request digest, and reviewed versions. `BEGIN IMMEDIATE` checks those
versions and the current lifecycle, candidate status, exact open WorkUnit,
Actor assignment and membership, workspace binding, and existing child
lineage. Lifecycle activation, metadata cleanup when present, child, start
event, WorkspaceLease, and human decision receipt commit together. Same-key
retries return the same child and skip provider dispatch; a new key cannot
create a second child from the consumed parent. An interrupted ambiguous
annotation write remains resolvable from the lifecycle reason plus the exact
durable candidate identity. The new receipt is excluded from the public event
projection.

`recovery_target_unavailable` remains deliberately fail closed: if no exact
open WorkUnit target can be proven, Execution recovery cannot select one, and
the existing Task cancellation path remains available. Timeout reconciliation
uses the exact selection route for ambiguous Manual candidates. Provider
effects remain outside SQLite; uncertain outcomes retain the admitted child
identity for existing crash or stalled-Execution reconciliation.

The focused audit also found that the dispatcher treated Auto as a fresh
re-execution even though CrashRecovery had assigned it only after proving an
exact reusable HarnessSession. That dropped the session link and could start a
second provider session after an uncertain result. Auto dispatch now admits a
child with that exact HarnessSession and resume snapshot. Its `BEGIN IMMEDIATE`
admission checks the cancelled parent and serializes duplicate dispatcher
attempts; an already committed exact child is returned as an idempotent replay
and is not started again. The dispatcher skips candidates whose target or
session no longer passes the resumability checks. Graceful shutdown now marks
an exact WorkUnit Execution Manual when it lacks a reusable session, so the
same explicit operator route can make the next decision. A Human may also
explicitly choose a fresh child for an Auto parent; automatic dispatch itself
does not fall back to a new provider session.

### Focused evidence

| Command or check | Result |
| --- | --- |
| Pre-fix `cargo test -p services --lib crash_recovery_with_two_manual_work_units_has_no_exact_recovery_route --locked --offline -- --test-threads=1` | PASS as reproduction: both exact individual routes were rejected and no durable child/event was added. |
| `cargo test -p services --lib crash_recovery_with_two_manual_work_units_can_be_resolved_by_exact_selection --locked --offline -- --test-threads=1` | PASS: A then B exact selection, same-key replay, no second child, immutable cancelled parents, lifecycle transition, human receipts, and retained lineage. |
| `cargo test -p services --lib explicit_recovery_same_key_after_post_commit_crash_replays_without_duplicate --locked --offline -- --test-threads=1` | PASS: startup CrashRecovery reconciles the committed Running child; same-key retry returns that now-cancelled child with one decision receipt and no new Execution. |
| `cargo test -p services --lib explicit_recovery_rejects_suspended_work_unit_actor_membership --locked --offline -- --test-threads=1` | PASS: current membership is rechecked at durable admission; a suspended Actor leaves lifecycle, annotation, Executions, and decision receipts unchanged. |
| `cargo test -p services --lib crash_recovery_mixes_auto_and_manual_policy_by_exact_work_unit --locked --offline -- --test-threads=1` | PASS: A Auto and B Manual coexist without an aggregate block; a durable Auto child consumes its parent candidate. |
| `cargo test -p services --lib graceful_shutdown_without_reusable_session_requires_an_exact_human_choice --locked --offline -- --test-threads=1` | PASS: exact graceful stop without HarnessSession remains Manual and an operator admits one exact child with start and decision receipts. |
| `cargo test -p services --lib human_may_explicitly_choose_a_fresh_child_for_an_auto_candidate --locked --offline -- --test-threads=1` | PASS: explicit Human choice creates a fresh child with a distinct pending HarnessSession and `Start`; Auto dispatch remains an exact-session `Resume`. |
| `cargo test -p services --lib three_work_unit_recovery_keeps_each_automatic_and_manual_identity --locked --offline -- --test-threads=1` | PASS: one resumable candidate and two distinct Manual WorkUnits remain independently recorded after selecting B. |
| `cargo test -p services --lib explicit_recovery_rejects_wrong_task_work_unit_and_stale_versions --locked --offline -- --test-threads=1` | PASS: foreign Task, wrong WorkUnit, stale Task version, and stale lifecycle version reject without mutation. |
| `cargo test -p services --lib competing_ambiguous_recovery_choices_commit_only_one_decision --locked --offline -- --test-threads=1` | PASS: incompatible concurrent A/B decisions produce one child and one receipt. |
| `cargo test -p services --lib explicit_recovery_rolls_back_lifecycle_child_and_receipt_together --locked --offline -- --test-threads=1` | PASS: injected receipt-write failure rolls back lifecycle, metadata, child, start event, lease, and decision receipt; retry then commits once. |
| `cargo test -p services --lib ambiguous_recovery_remains_selectable_if_annotation_write_is_interrupted --locked --offline -- --test-threads=1` | PASS: injected annotation-write interruption leaves a selectable exact candidate; choosing A preserves pending B. |
| `cargo test -p services --lib ambiguous_agent_timeout_recovery_uses_the_same_exact_selection_boundary --locked --offline -- --test-threads=1` | PASS: ambiguous timeout candidates preserve exact identities and an authorized exact choice resolves one. |
| `cargo test -p services --lib explicit_multi_work_unit_recovery_keeps_repository_workspaces_and_leases_separate --locked --offline -- --test-threads=1` | PASS: two repository WorkUnits recover with their own Workspace and active lease; cancelled parents retain no lease. |
| `cargo test -p services --lib ordinary_reexecute_ --locked --offline -- --test-threads=1` | PASS, 3 tests; ordinary exact recovery, snapshot failure, and WorkUnit-version race remain correct. |
| `cargo test -p services --lib follow_up_retry_replays_exact_child_and_new_key_authorizes_next_child --locked --offline -- --test-threads=1` | PASS: V133 follow-up receipt replay and new-key behavior remain unchanged. |
| `cargo test -p services --lib follow_up_on_sibling_work_unit_preserves_current_block_owner_and_evidence --locked --offline -- --test-threads=1` | PASS: an error or follow-up from a sibling cannot replace the current blocked Execution owner or clear its evidence. |
| `cargo test -p services --lib failed_external_start_after_durable_recovery_does_not_replay_old_attempt --locked --offline -- --test-threads=1`; `cargo test -p services --lib rejected_recovery_cancel_preserves_blocking_metadata --locked --offline -- --test-threads=1` | PASS: provider failure does not replay an old attempt; explicit Task cancellation still succeeds after a rejected transition is retried. |
| `cargo test -p api --lib legacy_review_and_agent_chat_events_are_not_public --locked --offline`; `cargo test -p api-types export_typescript --locked --offline -- --ignored` | PASS: recovery decision receipt stays private; generated `RecoverWorkUnitExecutionRequest` binding exports. |
| Initial `cargo test -p services --lib recovery::tests --locked --offline -- --test-threads=1` sweep; isolated reruns of the three reported cases | Sweep: 33 passed; one new test used a pre-PR13 `agent` table name and two existing tests had a stale timeout action assertion and omitted executor injection. Those test-only issues were corrected; the three affected tests each passed in isolated reruns. |

The `restart_failed_task()` contract was inspected with the current call-site
census. It has no REST, CLI, or in-repository production caller. It clears
Task-level `failed_json` but leaves lifecycle transition authority to the
existing lifecycle API; it does not resolve Execution-owned blocking
metadata. No change was justified in this focused closure.

### Residual risk and final census

- No route can recover an Execution whose exact open WorkUnit, current Actor,
  or workspace identity is unavailable. This is intentional fail-closed
  behavior; cancel the Task or create a new Task with valid scope.
- Provider start remains external. A lost response uses the same-key receipt
  and exact child; new execution from that parent is rejected once a child
  exists.
- Workspace-wide build/test/clippy, migrations, and global CI were not run.
  The branch CI state will be reported only from live GitHub evidence.

```text
P0 = 0
P1 = 0
P2 = 0
P3 = 0
BUG_STILL_LIVE = 0
```

This supplement supersedes the earlier multi-WorkUnit recovery status above.
PR13 remains subject to independent review; merge is not authorized.

## PR13 microfix audit — paused Agent automatic recovery — 2026-10-14

### Finding and reproduction

P2 reproduced: `compute_effective_status()` treated `Agent.paused` as
unavailable, and manual recovery rejected a paused Agent, but automatic
recovery could pass through dispatcher candidate selection, child preparation,
durable admission, and provider start without consulting that condition. The
regression fixture has an exact Auto parent and valid reusable HarnessSession.
In a controlled red run, temporarily removing the dispatcher, service
preflight, and durable-admission pause guards made
`paused_agent_keeps_auto_candidate_pending_until_unpaused` fail with one child
admitted (`left: 1`, expected `0`). The guards were restored and the same test
passes; the parent remains unchanged and the candidate stays queryable while
paused.

### Correction and boundary audit

The dispatcher skips a paused Agent before recovery work. The automatic
re-execution service checks again after loading the Agent, and
`start_execution()` rejects a paused Agent before provider dispatch. The
automatic durable-admission transaction also checks `agent_current.paused = 0`
after `BEGIN IMMEDIATE` and before changing lifecycle metadata or inserting a
child. A pause that wins before this write transaction causes the existing
version-conflict path; rollback leaves no child, preserves the Auto parent,
and keeps the candidate available. No domain state, production service, table,
or migration was added.

The existing V132 `work_unit_execution_admission_guard` checks active Role
membership and counts Running Executions for the Agent against
`max_concurrent_tasks` during the same child insert. `BEGIN IMMEDIATE`
serializes concurrent writers around that check, so automatic recovery does
not need a second capacity counter. The existing trigger error is now passed
through `work_unit_write_error()` so a capacity rejection remains the typed
`AgentAtCapacity` error. No V121–V133 migration was changed.

The route census found one automatic child writer:
`TaskDispatcher::recover_auto_cancelled_work_unit_executions()` to
`re_execute_automatic_recovery_execution()` to durable WorkUnit admission and
`start_execution()`. RoleMembership and capacity are checked by the existing
service/SQLite admission contracts; pause is checked at each relevant
boundary. Manual, mixed, and multi-WorkUnit recovery continue to use their
existing exact-target rules.

### Focused validation

| Check | Result |
| --- | --- |
| Controlled red run of `paused_agent_keeps_auto_candidate_pending_until_unpaused` with dispatcher, service-preflight, and durable-admission guards temporarily removed | Expected FAIL: one paused-Agent child was admitted (`left: 1`, expected `0`); the provider-frontier guard remained active and all files were restored immediately. |
| `cargo test -p services --lib paused_agent_keeps_auto_candidate_pending_until_unpaused --locked --offline -- --test-threads=1` | PASS: paused candidate creates no child; after unpause the exact candidate admits one child. |
| `cargo test -p services --lib automatic_recovery --locked --offline -- --test-threads=1` | PASS, 3 tests: exact candidate policy, stale pause rejected before commit, and SQLite capacity rejection. |
| `cargo test -p services --lib concurrent_dispatcher_ticks_resume_one_exact_auto_child_once --locked --offline -- --test-threads=1` | PASS: two dispatcher ticks admit one child and make one external start. |
| `cargo test -p services --lib paused_agent --locked --offline -- --test-threads=1` | PASS, 2 tests: candidate stays pending and `start_execution()` makes no provider call for a paused Agent. |
| `cargo test -p services --lib failed_external_start_after_durable_recovery_does_not_replay_old_attempt --locked --offline -- --test-threads=1` | PASS: a post-admission external start failure does not replay the prior attempt. |
| `cargo test -p services --lib explicit_recovery_same_key_after_post_commit_crash_replays_without_duplicate --locked --offline -- --test-threads=1` | PASS: same-key post-commit replay returns the one durable child without another start. |
| `cargo test -p services --lib ambiguous_recovery_remains_selectable_if_annotation_write_is_interrupted --locked --offline -- --test-threads=1` | PASS: exact ambiguous candidate remains selectable after an interrupted annotation write. |
| `cargo test -p services --lib ordinary_reexecute_ --locked --offline -- --test-threads=1` | PASS, 3 tests: exact manual recovery, snapshot failure, and WorkUnit-version race. |
| `cargo test -p services --lib explicit_recovery_rolls_back_lifecycle_child_and_receipt_together --locked --offline -- --test-threads=1` | PASS: lifecycle, child, and decision receipt remain atomic. |
| Focused `services` test-binary regressions: mixed Auto/Manual recovery, two- and three-WorkUnit selection, suspended membership rejection, concurrent admission, block-owner race, sibling follow-up, and multi-WorkUnit workspace/lease separation | PASS: each exact test passed. |

The final census for this microfix is:

```text
P0 = 0
P1 = 0
P2 = 0
P3 = 0
BUG_STILL_LIVE = 0
```

PR13 remains ready only for independent review after the branch push and
remote-ref verification. Merge remains unauthorized; PR14 has not started.
