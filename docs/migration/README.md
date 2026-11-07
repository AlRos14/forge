# Migration ledger index

The ordered PR0–PR15 migration is complete and merged. The minimum verified
base for the current runtime is `origin/main` at
`7fb78d9e845869122e61cce5cb510a3b0853afc9` (PR #20). Individual documents below
record the state and evidence at their named baseline; phrases such as "not
merged", "ready for review", or "subject to review" describe that historical
moment, not the current repository.

| Document group | Classification | Use |
|---|---|---|
| [architecture-v2.md](architecture-v2.md) | **Current invariant contract** plus labeled historical snapshots | The INV-001–INV-040 authority. Baseline tables and per-PR implementation notes are historical evidence. |
| [history-v071-v076-project-os.md](history-v071-v076-project-os.md) | Historical | Explains preserved Project OS history. It is not a current workflow. |
| `plan-pr1` … `plan-pr15` | Historical implementation ledgers | Preserve decisions, baselines, and acceptance evidence for each migration step. Do not use them as current API or operations guides. |
| `pr-0a.md`, `pr-1.md` … `pr-3.md` | Historical exit ledgers | Record earlier workstream exits. |
| [plan-pr15-acceptance-report.md](plan-pr15-acceptance-report.md) | Historical acceptance evidence | S01–S13 evidence at its pinned SHA. |
| [post-v2-hardening-acceptance.md](post-v2-hardening-acceptance.md) | Current post-V2 hardening ledger | F-02–F-05 repairs, new findings, and A/B/C evidence boundaries. |
| [plan-task-driven-workunits.md](plan-task-driven-workunits.md) | Implementation plan and product contract | Task launch, automatic dispatch, exact review subjects, rework, and purpose retirement. |
| [task-driven-workunits-implementation.md](task-driven-workunits-implementation.md) | Current implementation ledger | V141–V144, service/API/MCP/CLI/UI changes, and validation performed. |

For current behavior use [architecture.md](../architecture.md),
[concepts](../concepts/), [api.md](../api.md), [cli.md](../cli.md), and
[getting-started.md](../getting-started.md).

## Proposed independent follow-up plans

These are implementation proposals, not current runtime contracts:

- [Plan A: roles, capabilities, and correction routing](../plans/roles-capabilities-and-rework.md).
- [Plan B: UI consistency and layout](../plans/ui-consistency-and-layout.md).

Each plan includes a separate agent brief, file ownership, acceptance criteria,
and a final integration boundary. Neither authorizes changing live Tasks.

## Accepted follow-up product direction — implementation pending

- [Task operating model](../concepts/task-operating-model.md): owner-approved
  responsibility and UX contract, distinct from current runtime behavior.
- [Task coordination and guided experience](../plans/task-coordination-and-guided-experience.md):
  whole-system boundary review, bootstrap decision, implementation slices and
  shared backend/UI acceptance.
- [PR0 — Task coordination contract](../plans/pr0-task-coordination-contract.md)
  and [ADR 0016](../adr/0016-task-coordination-turn.md): the accepted
  coordination-start contract, authority matrix, record shapes, recovery rules
  and UX states. PR1–PR6 remain implementation work.
