# Task-driven WorkUnits implementation ledger

Status: Implemented on branch `fix/post-v2-security-hardening`, not merged or
released. Baseline before implementation:
`76c94452 docs: plan task-driven WorkUnits and retire execution purpose`.

## P0 — contract and inventory

- Added [ADR 0014](../adr/0014-task-launch-and-purpose-retirement.md).
- Classified Execution purpose readers/writers across REST, MCP, CLI, web,
  services, DB, Gates, review, planning, orchestration, tests, and migrations.
- Preserved historical purpose values before removing the active column.

## P1 — durable Task launch

- V141 adds `task_dispatch`, `task_dispatch_receipt`, and immutable receipt
  guards.
- `TaskLifecycleService` exposes launch preflight, launch, pause, resume, and
  cancellation receipt operations with exact Task and TaskLifecycle versions
  and idempotency.
- `TaskDispatcher` admits exact ready Agent WorkUnits while the intent is
  `running`, respects Agent capacity and a bounded per-Task ceiling, and leaves
  Human assignments as pending human work.
- A completed Execution leaves an open WorkUnit `awaiting_acceptance`; the
  dispatcher does not create another automatic attempt.
- REST, MCP, and CLI expose Task launch/pause/resume/cancel. Manual Execution
  creation returns `410 Gone`.
- `POST /work-units/{id}/workspace` prepares repository WorkUnits explicitly.
- V144 adds optional WorkUnit `execution_instructions`.
- V145 closes pause and per-Task concurrency races inside Execution admission.

## P2 — deliverables, review, and dynamic correction

- V143 adds `task_rework_receipt`, `work_unit_result_requirement`, and
  `work_unit_review_subject`.
- `TaskReworkService` validates an exact request-changes ReviewReport, creates
  correction plus new reviewer WorkUnits, records exact result/review
  subjects, checks contributor independence, and consumes a bounded eight-round
  budget atomically.
- `POST /tasks/{id}/rework` and `forge_request_rework` expose the operation.
- Review remains the reviewer Role plus its exact ReviewReport output.

## P3 — public retirement

- Removed `ExecutionPurpose` from REST, MCP, CLI, DTOs, generated web types,
  events, and runtime checks.
- Removed the manual Execution launch form from the Task UI.
- Added Task dispatch controls, blocker display, WorkUnit dispatch states,
  `awaiting_acceptance`, and harness instructions.
- Activity and diagnosis retain Execution history; users do not configure an
  Execution form.

## P4 — persistence and cleanup

- V142 copies exact purpose values to `pr13_execution_purpose_history`,
  rewrites purpose-dependent triggers/views, drops `idx_execution_purpose`,
  and removes `execution.purpose`.
- New mutable records are included in guarded Project teardown.
- Historical Executions, sessions, leases, outputs, receipts, and migration
  ledgers keep their IDs and relationships.

## Earlier implementation validation

The checks below belong to the earlier implementation snapshot. They are not
acceptance evidence for subsequent review fixes or a future commit.

- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo fmt --all -- --check`
- focused DB migration test for V141–V144
- API happy path, e2e, daemon onboarding, remote roundtrip, WorkUnit, Task
  dispatch, rework, and PR15 reference scenarios
- `pnpm lint`, `pnpm typecheck`, `pnpm test`

Full workspace test/CI status must be re-established on the final HEAD before
merge. This ledger does not claim a merge, release, or deployment.

## Reliability correction scope — 2026-11-06

The follow-up changes preserve exact recovery priority over fresh dispatch,
extend transactional pause/capacity guards to parented WorkUnit attempts, and
compare cancellation request identity on replay. Review preparation now names
its source WorkUnit before a result exists; the frozen admitted result is
separate from mutable preparation versions. Independent review must survive
later allocation and concurrent admission, and cannot proceed with a missing
result. Initial Human review submission is a WorkUnit operation, with Forge
creating its provenance attempt internally. Lifecycle/SSE changes invalidate
launch readiness in the UI. Focused acceptance results are recorded after
these changes have been integrated and tested.

## Resumed validation — 2026-11-06

Continuation of Codex session `01a172fc-548f-7951-a842-2f17ef778ad5`.
V148 restricts recovery reservation to the exact current open WorkUnit version,
role, and assigned Actor, preserving interrupted attempts as history after an
allocation change. Candidate queries use the same predicate as admission.

Touched invariants: INV-008 (historical attempt), INV-010 (session continuity),
INV-016 (exact WorkUnit scope), INV-018 (independent review), INV-030
(deterministic authority), and INV-034 (durable authority).

Current verified checks:

- PASS: frontend typecheck and 17 tests across work management, rework, Human
  review, Task detail, hooks, and SSE.
- PASS: stale interrupted allocation regression in SQLite, including migration
  application and fresh admission of the new revision.
- PASS: both API happy-path tests, three Task dispatch tests, and both rework
  tests with reviewer WorkUnit subjects.
- PASS: full repository result -> Human request-changes -> initially unassigned
  correction -> later allocation/instruction edit -> completion -> independent
  Human pass cycle, with exact result Execution and admitted version asserted.
  Its Gate assertions accept the exact result and reject a different result pin.
- PASS: SQLite pause and four-attempt ceiling guards for parented recovery.
- PASS: concurrent dispatcher recovery admits one child and preserves continuity.
- PASS: frontend lint, Rust format check, and git diff whitespace check.
- NOT RUN: full workspace test/CI, browser E2E, and real provider acceptance.

The rework fixtures now create reviewer WorkUnits and submit reports only after
source acceptance. The review-cycle fixture uses the current Gate request
schema and preserves the shell command exit status before appended cognitive
context. The orchestrator policy fixture includes the new optional subject.

Only regenerable build artifacts were removed for disk space. A concurrent
compile lost references to removed libraries; Cargo rebuilt those artifacts
before the backend checks were repeated. No installed data or workspaces were
removed. Validation above precedes the user-authorized commit, push, and local test restart.
No merge, release, or production deployment is claimed.
