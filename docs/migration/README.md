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

For current behavior use [architecture.md](../architecture.md),
[concepts](../concepts/), [api.md](../api.md), [cli.md](../cli.md), and
[getting-started.md](../getting-started.md).
