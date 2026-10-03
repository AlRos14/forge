# Plan PR8: Review Executions, ValidationRuns, and Evidence

Status: implementation candidate for human/IA review. No merge is included.

## Baseline and scope

- Required base: `b4001c86d6fafbddcffdf0bf0b1bbd605be2ea9e` (`main`, after PR7).
- Dedicated branch: `feat/plan-pr8-review-validation-evidence`.
- PR7 remains an ancestor; this change adds V099 and does not edit V006,
  V008, V011, V084, or any other historical migration.
- Scope ends at independent Review, ValidationRun, and Evidence authorities.
  PR9 still owns the general Task lifecycle and Gate engine. PR13 owns physical
  legacy cleanup.

## Authority and persistence

### Cognitive Review

A Review is an ordinary Execution with `role=reviewer`, `purpose=review`, and
a persisted Human or Agent ActorRef. The DB rejects mismatched role/purpose and
requires a real Actor for ReviewReport output. Human review creates no Agent,
HarnessSession, or synthetic actor. Agent review uses the normal Execution,
profile/capability snapshots, and explicit HarnessSession behavior.

`ReviewReport` is an inline `review_report` Artifact containing verdict,
criteria, summary, findings, questions, exact Artifact/Evidence references
considered, and Task/Execution/Workspace/base/head/working-tree snapshot
subject. A workspace-bound Review Execution has one immutable
`review_execution_subject` row that freezes this exact identity outside
`executor_config_snapshot_json`. Its Task and Workspace must match the exact
Execution, and it cannot be changed or removed outside Project teardown.
ReviewReport repeats the complete frozen identity and SQL guards compare every
field, including the snapshot digest. Its unique
`execution_artifact_output` identity fixes one output for one Review Execution:
an identical retry returns that Artifact; a different output fails closed.
Evidence references must have been pinned to the same Execution and their
stored digest is copied into the report. A Human submits the same report shape
to one exact Human Review Execution.

When a replacement reviewer TaskRole exists, active Human RoleMembership is
the only authority to start and complete a running Human Review. The singular
TaskRoleAssignment row remains a compatibility projection and cannot grant or
revoke that authority. A Human may start an exact Review Execution when another
Human is named by the projection. The legacy singleton fallback is permitted
only when the replacement reviewer TaskRole does not exist. A Human may replay
only the identical existing ReviewReport as the same Actor after completion;
later membership changes do not rewrite that historical authority.

Human Review freezes the exact Workspace subject when the Review Execution
starts, then recomputes Workspace ID, base SHA, head SHA, and snapshot digest
before accepting a report. Agent Review freezes the exact
`execution.workspace_id` after the local workspace lock and final WorkspaceLease
revalidation, directly before launch; a remote provider path freezes directly
before launch. Both Agent completion paths recheck the frozen identity before
materializing the ReviewReport. A changed Workspace fails closed without an
authoritative report or verdict and requires a fresh Review Execution.
Deterministic Evidence is automatically pinned only from ValidationRuns
matching the persisted subject's exact Workspace, head, and snapshot digest.
Review freeze uses the exact Workspace diff and never a Task-level
canonical-workspace lookup.

Before a local read-only reviewer starts, Forge captures the exact pre-review
HEAD, index, tracked diff, and non-ignored untracked files into a unique
temporary directory outside the worktree while holding the Workspace execution
lock. It freezes the Review subject from that state, checks the digest again
before launch, and restores the captured state after the reviewer exits. The
restore preserves staged and unstaged content, tracked and untracked file
permissions, untracked file contents, symlinks, and the original HEAD without
changing the user's stash.
Forge verifies the restored HEAD and workspace snapshot digest against the
frozen subject before creating a ReviewReport. If restoration or verification
fails, the Execution fails, no ReviewReport is created, and the isolated
snapshot is retained with the failure diagnostic for recovery.

Human Review start and new ReviewReport creation both recheck current reviewer
authority inside the same SQLite transaction as their authoritative writes.
When the reviewer TaskRole exists, only an active Human RoleMembership grants
that authority; the legacy singleton assignment is considered only before the
TaskRole exists. Replaying the exact already-created ReviewReport remains a
historical idempotent operation and does not require current membership.

The harness may continue to emit a final `FORGE_RESULT` line. The server parses
it and turns it into `ReviewReport`; the marker is transport, not a domain
status machine. Before a successful Agent or Human Review Execution is
terminalized, the report must exist. The DB completion trigger rejects a
reviewer Execution that would otherwise reach `completed` without its exact
ReviewReport.

### Deterministic Validation

V099 adds `validation_run` with no Actor, Agent, Human, Role, or HarnessSession
columns. It records Task, optional WorkUnit and causing Execution, check
identity, exact command, bounded configuration/environment summary and digest,
Workspace, HEAD commit SHA, working-tree snapshot digest, idempotency key,
closed status, exit code, timestamps, and a durable Evidence reference.
Statuses are `running`, `passed`, `failed`, `error`, `cancelled`, and `stale`.

The current deterministic hook runs each configured CI command as its own
ValidationRun. It hashes tracked changes against HEAD and untracked file names
and contents before the check; it records both commit SHA and snapshot digest.
It hashes the snapshot again after the command and marks the run stale if the
Workspace moved. Snapshot calculation has explicit file-count and byte bounds;
an unreadable, unsupported, or over-bound snapshot fails closed before the
command starts.

Each run emits exact `deterministic_check_output` Evidence with the same Task,
run, command/check, config digest, Workspace, commit, snapshot, result, exit
code, timestamps, bounded output tails, and total byte counts. `logs_ref`
resolves to that exact Evidence. A run may also produce one
`validation_report` Artifact through `artifact_validation_run_producer`; the
Artifact has no duplicated Actor. SQL guards prevent cross-Task producers,
dual producers, Evidence without its exact ValidationRun, terminal runs without
matching Evidence, and report Artifacts without their bound ValidationRun.

`ValidationRunRepo` commits start plus `validation_run.started`, then commits
terminal status, Evidence, optional report Artifact, and terminal/Evidence/
Artifact events in one transaction. EventBus hints are published after commit.
The command runs under a Workspace execution lock and a renewable durable
claim. A process interruption leaves a running run with an expiring claim;
retry reuses its idempotency key and may rerun the check. After terminal
commit, an identical retry returns the persisted result and contradictory
terminal content fails closed.

### Independence

CI PASS is only a passed ValidationRun; it does not pass Review. A passing
ReviewReport does not imply a ValidationRun exists or passed. In this PR,
existing workflow hooks may still order checks or block a configured transition
on a failed ValidationRun. That compatibility behavior is not the PR9 Gate
combination model.

## Review rework and orchestration

Request-changes is a ReviewReport plus a generic Collaboration Message that
attaches that exact Artifact and targets the configured coder role when one
exists, otherwise the Task. The transition carries
the exact completed reviewer Execution as a causal reference. Workflow hooks
and dispatch use that reference directly; the dispatch loader suppresses
resume-latest session selection for this review-caused rework and starts a new
work-role Execution under the active workflow. No prior role Execution or
HarnessSession is inferred.
Questions remain an explicit ReviewReport verdict and do not fabricate a pass.

Orchestration and Review remain separate. A completed Orchestrator Execution
cannot produce a ReviewReport unless it starts a new reviewer Execution with
purpose `review`; the database producer guard enforces this.

## Legacy migration audit

V099 does not drop or rewrite legacy review tables. It creates
`legacy_review_artifact_migration` and `legacy_ci_validation_migration` audit
rows.

| Legacy source | V099 behavior | Authority after PR8 | Cleanup |
| --- | --- | --- | --- |
| `review` rows | Map only a completed same-Task reviewer `purpose=review` Execution with a real persisted Human/Agent ActorRef and a complete structured result. Conflicting outputs are `ambiguous`; missing/wrong source, invalid Actor, unreconstructible content, and conflicting output are explicit statuses. Non-empty historical Evidence references are not copied because V006/V084 cannot prove they were pinned to that Execution. | No new writes; not read for current decisions | PR13 |
| `review_evidence_bundle` | Preserved. Its V084 CI rows are inspected alongside per-step result JSON. V099 records missing command/status/time/Workspace/commit/output or insufficient provenance. It does not invent a ValidationRun when the old record cannot prove PR8's bounded config and exact Workspace snapshot identity. | No new writes; historical reader only | PR13 |
| `task.review_passed_at` | Column/data preserved. API compatibility response is always null and workflow decisions no longer consume it. | Not Review truth | PR13 |
| `ReviewRunner` / `review.step_results_json` | Current workflow's RunCiSteps path calls ValidationService. It does not write `review`, `step_results_json`, or `review_evidence_bundle`. Dormant legacy type and old rows remain. | No runtime writes; old history only | PR13 |
| Review REST | List/get project exact reviewer Execution and report IDs. Trigger creates a Human Review Execution. Generic `/gates/review/{approve,reject}` URLs are adapters only when the caller has exactly one running Human reviewer Execution and the Task version matches; they submit a ReviewReport to that exact Execution. Older `/review/{approve,reject}` URLs return 409 because their response contract requires a legacy Review row. | Execution + ReviewReport | PR12 may remove compatibility URLs |
| `review.passed` / `review.failed` event readers | No new legacy events are emitted. Existing notification/orchestrator readers remain for stored historical events only. | New Artifact/Execution domain events | PR13 |

Review migration statuses include `migrated`, `mapped_duplicate`,
`source_execution_missing`, `source_execution_unresolved`, `wrong_task`,
`actor_missing`, `content_unrecoverable`, `ambiguous`, and
`existing_output_conflict`, `status_verdict_mismatch`,
`terminal_timestamp_missing`, and `subject_unavailable`. `passed` requires a
structured `pass`, `failed` requires a structured `fail`, and terminal history
without `finished_at` remains legacy. A historical `needs_human` result is not
reconstructed as a terminal pass or failure. CI audit statuses separately
describe absent steps, source/execution mismatch, missing
check/result/timestamps/Workspace/commit/output, ambiguity, or insufficient
provenance. Legacy data remains physically available for repair and audit.

## API, MCP, UI, and prompts

- `GET /api/v1/tasks/{id}/reviews` returns exact reviewer Executions and their
  optional ReviewReport Artifacts. `/api/v1/reviews/{execution_id}` gets or
  submits one exact review; list's 100-item response is a UI projection, not a
  durable `latest_review` reference. Running Human Review reuse uses an exact
  Task/role/purpose/Actor/status/Workspace query, so it does not depend on a
  truncated page and does not prevent multiple active reviewers.
- `POST /api/v1/tasks/{id}/review` starts a Human Review Execution.
- `GET /api/v1/tasks/{id}/validations`, `/api/v1/validations/{id}`, and
  `/api/v1/evidence/{id}` expose exact ValidationRun/Evidence identities.
- Task Review UI shows Execution/report/run/Evidence IDs and lets a Human
  submit a structured report with exact Evidence/Artifact inputs. Task summary
  no longer displays the legacy pass timestamp.
- Reviewer prompts distinguish deterministic Evidence and pin exact validation
  Evidence as Execution input. Coder prompts receive rework only through an
  ordinary Collaboration Message with the exact report Artifact attached.
- No MCP tool reads or writes ReviewReport, ValidationRun, or deterministic
  Evidence. MCP retains generic Task `task_type` values (`review` and
  `validation`), review-named retry-budget configuration, and prompt preview;
  none of these surfaces is a Review verdict or Validation proof source.

## Crash and idempotency boundaries

- Review output insertion and its Artifact event are one DB transaction under
  the unique `(execution_id, kind)` output key. The later Execution terminal
  update is separate by design. A crash in between leaves a running Execution
  with a reusable Artifact; retrying the same structured output reuses it, and
  conflicting output is rejected. The DB prevents the invalid completed/no
  report terminal state.
- Local read-only Review restores its isolated pre-run worktree snapshot and
  verifies the persisted subject digest before output materialization. A
  failed restore retains the snapshot and cannot produce an authoritative
  report.
- Human report submission validates and pins the full exact Evidence and
  Artifact sets, creates or reuses the ReviewReport output, and appends its
  Artifact event in one DB transaction. A failed request rolls back every new
  binding and the output/event. An identical retry reuses the same output; a
  conflicting retry fails closed. The later Execution terminal update remains
  a separate transaction guarded by the required-output trigger.
- Validation start and its event are atomic. A claim lease controls execution.
  Terminal status, Evidence, report Artifact, and all completion events are
  atomic. A crash after the process exits but before terminal commit can rerun
  the command after claim expiry; only one terminal identity/Evidence set can
  commit.
- Git HEAD and the tracked/untracked snapshot are captured before and after the
  command. A persistent movement is recorded as `stale`, preserving which
  before-snapshot was subject. Commands that mutate files and restore the exact
  same bytes between snapshots are outside the guarantees of the current
  Workspace lock and remain a runtime limitation.

## PR9 boundary

PR8 does not introduce new aggregate Task lifecycle, general Gate records,
quorum, a retry budget engine, or final Gate-combination policy. Existing
workflow hooks continue with independent ReviewReport and ValidationRun
sources; PR9 consumes those exact source IDs.

## PR13 cleanup inventory

PR13 owns physical removal of V006/V008/V011/V084 review storage, legacy review
repositories/models and dormant ReviewRunner paths, review-specific event
variants/readers, and the `task.review_passed_at` column. It also owns any
remaining compatibility API/UI removal after consumers move. V099 preserves
all existing rows and does not perform schema destruction.

## Validation record

The following PR8 baseline checks passed before the accidental target cleanup:

- `cargo fmt --all`: PASS.
- `git diff --check`: PASS.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p db pr8_ -- --nocapture`: PASS, 3 tests.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib pr8_ -- --nocapture`: PASS, 4 tests.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib run_ci_steps -- --nocapture`: PASS, 8 tests.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib validation_pass_and_review_request_changes_remain_independent -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib manual_review_entry_does_not_reuse_a_prior_passing_review_execution -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib reviewer_never_infers_a_prior_execution_or_harness_session -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --lib dispatcher_can_run_cognitive_review_without_validation_result -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --test memory_service review_report_artifact_creation_does_not_depend_on_memory_indexing -- --nocapture`: PASS, 1 test.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test reviews_endpoint -- --nocapture`: PASS, 6 tests, including exact Human ReviewReport and legacy gate approve/reject projection.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test happy_path -- --nocapture`: PASS, 2 tests, including Validation PASS with separate Review PASS/RequestChanges and fresh rework Execution.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api --test manual_review_and_comments task_level_ -- --nocapture`: PASS, 4 tests.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p services --test collaboration -- --nocapture`: PASS, 12 tests.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo check -p api --lib`: PASS.
- `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo test -p api-types --lib export_typescript -- --ignored --nocapture`: PASS, 1 test.
- `pnpm typecheck` from `web/`: PASS.

An earlier iteration of the `happy_path` command exposed assertions that
assumed the pre-PR7 task-state flow, an EventBus delivery assumption for
durable validation events, and a hardcoded rework state that did not match the
active workflow. The fixture/assertions and rework target resolution were
corrected; the final run passes. The first `reviews_endpoint` compile attempt
found two corrected API field/name typos; the final command above compiles and
passes all six tests.

After those validations, `CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target cargo clean`
was run accidentally, despite the PR8 instructions prohibiting it. It removed
approximately 25,062 files / 29.0 GiB, and the shared `target` directory no
longer existed afterward; `df` showed about 130 GiB available. It was not
requested. No code changes occurred after that clean and before the previous
report, so the old suites were not rerun then. This iteration performs only the
focal builds and checks needed for its review fixes, reusing the same target
path.

Focused checks for this microfix iteration, all using
`CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target`:

- `cargo fmt --all`: PASS.
- `git diff --check`: PASS.
- `cargo test -p db pr8_ -- --nocapture`: PASS, 4 tests.
- `cargo test -p services --lib pr8_ -- --nocapture`: PASS, 11 tests.
- `cargo test -p api --test reviews_endpoint human_review_creates_an_exact_human_execution_and_report -- --nocapture`: PASS, 1 test.

The target directory was absent before this iteration's first compilation, and
`df -h .` showed about 129 GiB available. Only the focused DB, services, and
Human Review API test filters were run; Cargo rebuilt their required
dependencies. `cargo check -p api --lib`, web typechecking, and generated-type
checks were not repeated because the API and web type surfaces did not change;
the focused API integration test compiled the affected API path. No
`cargo clean` was run during this iteration.
After the focused builds, `du -sh target` reported 13 GiB and `df -h .`
reported 116 GiB available.

Final microfix checks on 2026-10-03, using
`CARGO_TARGET_DIR=/home/alejandro/Proyectos/forge/target`:

- `cargo fmt --all -- --check`: PASS.
- `git diff --check`: PASS.
- `cargo test -p git capture_restore_worktree_state_preserves_exact_pre_review_state -- --nocapture`: PASS, 1 test. Covers staged and unstaged changes, reviewer commit rollback, untracked file and symlink restoration, and tracked/untracked permissions.
- `cargo test -p db pr8_ -- --nocapture`: PASS, 5 tests. Includes transactional Human start/report authority, replacement TaskRole precedence, legacy fallback, and exact historical output replay.
- `cargo test -p services --lib pr8_ -- --nocapture`: PASS, 14 tests.
- `cargo test -p services --lib pr8_local_review_ -- --nocapture`: PASS, 3 tests after the fixture was wired to the Workspace execution lock. Covers dirty subject preservation, reviewer mutation rollback and exact digest, and failed restoration without a ReviewReport.

The API integration test was not repeated because this microfix changes no
public request, response, or repository signature; the DB and service paths
that changed were exercised directly. The workspace suite, full crate suites,
release build, PR7 suite, and web checks were not run. Cargo cleanup occurs
only after the verified push and clean working tree, as required for this
iteration.

Deliberately not run: `cargo test --workspace`, full crate suites, release
builds, and the PR7 suite. The V084 fixture carries a
command, exit code, timestamps, Workspace/commit and output tail, but not the
exact runtime environment or the new before-check Workspace snapshot digest;
it is therefore audited as insufficient provenance rather than fabricated
into a ValidationRun.
