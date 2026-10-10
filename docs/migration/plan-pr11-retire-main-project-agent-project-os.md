# Plan PR11 — Retire Main Agent, Project Agent, and Project OS

## Scope

PR11 ends Main Agent and Project Agent as runtime classes and removes the
Project OS vertical as an authority. Agents remain ordinary harness-bound
Actors. TaskRole and RoleMembership define Task responsibility; Execution and
HarnessSession preserve who performed work and which harness session was used.
The event-driven `OrchestratorRuntime`, Task lifecycle, Gates, ValidationRuns,
Evidence, WorkUnits, WorkspaceLease, and shared media storage remain in the
runtime.

The central contract is:

> The orchestrator owns work. The harness owns cognition.

> Project bindings do not grant Task authority.

PR11 stops writes and authority. PR12 removes or redesigns public REST, MCP,
CLI, web, generated-type, and public-event surfaces. PR13 removes legacy tables,
columns, and compatibility code after its upgrade fixtures pass. PR11 does not
drop tables, erase historical events, or rename public products.

## Target architecture

```text
Project
  └── Task
       ├── TaskRole / RoleMembership
       ├── WorkUnit
       ├── Execution / HarnessSession
       ├── Artifact / ValidationRun / Evidence / Gate
       └── Message / Handoff / Proposal / Decision

Agent = ActorRef::Agent + harness identity + profile revision
        + explicit TaskRole membership + ordinary Execution
```

There is no project-wide implicit Agent role and no project binding that can
select an Actor, profile, responder, permission, scheduler candidate, or
Execution identity. An Agent can hold several Task roles, and each role can
have several Agents. A Task may be created without any Agent assignment.

## Source-of-truth matrix

| Concern | Authority after PR11 |
| --- | --- |
| Agent identity and profile | Agent identity and HarnessProfileRevision |
| Project membership | Human ownership/membership and generic Agent ownership/visibility rules |
| Task responsibility | TaskRole + RoleMembership |
| Cognition | External Harness through an Execution |
| Continuity | Explicit HarnessSession attached to an Execution |
| Work scope | Task + WorkUnit |
| Planning and design | `Execution(purpose=plan)` + Artifact with exact producer |
| Implementation | `Execution(purpose=implement)` |
| Review | `Execution(purpose=review)` + review Artifact |
| Deterministic validation | ValidationRun + Evidence |
| Communication | Message |
| Delegation | Handoff |
| Proposed disruptive action | Proposal |
| Authorization decision | Decision tied to a real Proposal |
| Orchestration | Orchestrator TaskRole + Execution + domain events |
| Readiness | Current Gate evaluation over exact inputs |
| Task lifecycle | `task_lifecycle` and immutable transitions |
| Workspace authority | WorkspaceLease, WorkUnit, and exact Execution snapshot |
| Main/Project and Project OS history | Read-only legacy records through PR12; physical cleanup in PR13 |
| Schema cleanup | PR13 |
| Old public surface removal | PR12 |

## Replacement and compatibility matrix

All rows use `dual-write = NONE`. The old and new systems never synchronize.
Where the new side has no exact Task-scoped equivalent, the old record remains
historical. A current binding, current profile, latest Execution, nearby
timestamp, or matching text is not provenance.

| Old representation | Old writer / reader | New writer / reader | Authority after cutover | Divergence protection | History, cleanup, rollback |
| --- | --- | --- | --- | --- | --- |
| `account_main_agent_binding` | Main Agent setup services, API, MCP; Main Chat and Genesis admission | No replacement writer; Agent identity/profile readers remain | Historical only; cannot choose responder, profile, Execution, project creation, or permissions | Stop public mutations and reject direct new rows; remove account binding from all admission queries | Preserve rows/events through PR12; drop in PR13. Additive cutover is not reversed by restoring an old binary. |
| `project_agent_binding` | Project Agent setup and action services, API/MCP; chat, scope, capacity, and permission readers | TaskRole/RoleMembership writes for a specific Task; generic Actor eligibility reads | Historical only; never grants Task scope or WorkspaceLease | Stop binding writes; remove binding from scope, permission, dispatch, and lease predicates; reconcile binding-only memberships | Preserve bindings. End only active/suspended memberships whose Agent has no independent global/account eligibility. Keep all other memberships. Physical removal is PR13. |
| `agent_chat`, messages, turn jobs, instruction revisions | Agent Chat service/routes/MCP and turn worker | Task Message/Handoff only when a real Task and exact Actor provenance exist | Historical transcript only; no cognitive runtime | Stop all chat mutations and turn claims; remove `AgentChatTurnWorker`; SQL guard prevents nonterminal job resurrection | Keep transcripts and job state without fabricating success or response. Pending jobs stay historical and unclaimable. Drop in PR13. |
| `agent_handoff` | Main-to-Project Chat handoff orchestration | Task-scoped Handoff for future Task work | Historical only unless an exact Task, sender, and target are already proven; no automatic conversion | Stop vertical writers; no Project Chat delivery path | Preserve old handoffs. No synthetic Task Handoff. Cleanup in PR13. |
| `product_genesis_session` | Main Agent Genesis routes/service/actions | Human creates a Project through ordinary Project authority; planning is an Execution on a real Task | Historical lifecycle only | Stop Genesis start/advance/cancel/handoff writes and all Genesis-triggered Project creation | Preserve active and terminal sessions as they are; do not continue or fabricate a Project. Cleanup in PR13. |
| `agent_action`, approval, and execution rows | Coordination/AgentAction services, Main/Project materializers, REST/MCP | Proposal → Decision and ordinary Task/Project operations, only for new exact Task-scoped work | Historical only; approved legacy actions cannot replay | Stop writers; reject execution and approvals after cutover; no AgentAction/Proposal dual-write | Existing Tasks created by an old action remain ordinary Tasks. Keep action history through PR12; cleanup in PR13. |
| `agent_commitment`, evidence, transfer | Commitment service and `CoordinationOutcomeConsumer` | Task, WorkUnit, Message, Handoff, and domain events | Historical only; an existing `originating_task_id` remains a reference to that Task | Stop service writes and remove outcome consumer startup/shutdown | Preserve linked and unlinked records; do not create Tasks or Handoffs. Cleanup in PR13. |
| `agent_inbox_item`, `agent_question` | Inbox/question services, action and outcome consumers | Message/Handoff and event-driven Orchestrator | Historical only | Stop writes and consumer projections; direct SQL insertion is fenced | Preserve unread/read history. No synthetic response or Task. Cleanup in PR13. |
| Agent semantic memory: `memory_item`, lifecycle assertions, source bindings, context manifests, `agent_lcm_*`, Agent scopes | Chat-memory consumer, memory/context services, memory routes | No replacement memory subsystem; exact Task outputs may use Artifact through a real producer | Historical only; no cognition, scheduling, or permission authority | Remove `AgentChatMemoryConsumer`; stop semantic memory writes and indexing; block retired writes | Preserve bodies, provenance, timeline, and audit rows. Never convert memory text into Artifact without exact Task and producer. Cleanup in PR13. |
| Attention, projections, and `agent_wake_*` | Attention/Mission Control projection worker and wake admission | Generic event-driven Orchestrator over TaskRole, Execution, and domain events | No Attention authority or wake runtime | Remove worker startup/shutdown and binding/commitment/chat readers from active orchestration | Historical projection/wake records remain readable where compatibility requires. Cleanup in PR13. |
| Operating skills | Main/Project Agent skill services and binding protocols | Native harness/repository skills; no Forge-owned cognitive protocol | Historical configuration only | Stop runtime lookup and writes where used only by the retired protocol | Preserve revisions. This does not remove harness-native skills. Cleanup in PR13. |
| Project Charter, revisions, approvals, amendments | Main orchestration and Charter routes/materializers; Task admission, Project setup, scheduler and workspace guards | Planning/design Artifact from a real Task Execution when useful | Historical only; cannot admit/block a Task or Execution, choose an Agent, or determine readiness | Remove Charter reads from all authorization/admission paths; new Project defaults do not require setup | Preserve exact records and approval receipts. Do not generate replacement Artifact. Cleanup in PR13. |
| Project Documents, revisions, approvals | Project Agent materializer and document routes; overview/baseline readers | Artifact from an exact producing Execution or ValidationRun | Historical unless exact Task, real producer, content/digest, and Actor provenance are proven | Stop writers and remove Project Document authority; no generic Artifact without provenance | No blanket conversion. Preserve records through PR12; cleanup in PR13. |
| Project Decision candidates, decisions, and links | Project Agent action materializer/routes; readiness, waiver, and release readers | Proposal → Decision tied to an exact Task and explicit decider | Historical only; cannot satisfy a current Gate, including old waiver/policy rows | Stop writes and remove current Gate/readiness reads; no retroactive Proposal or Human Decision | Preserve outcomes and decider evidence unchanged. Cleanup in PR13. |
| Project Execution Baseline and revisions/approvals | Project Agent materializer and baseline routes; Task governance, scheduler and WorkspaceLease guards | Task/WorkUnit/TaskRole/Gate and immutable Execution snapshots | Historical only; cannot admit or block new Task execution | Stop baseline writers and remove baseline/Charter predicates from Task and lease authority | Keep baseline JSON and approval receipts. No Baseline V2. Cleanup in PR13. |
| `project_task_governance` | Task creation/subtask services and baseline refresh; Task admission, capability, and lease guards | Task lifecycle, TaskRole, WorkUnit, Gate, Execution admission, WorkspaceLease | Historical only; neither a positive nor negative admission input | Stop inserts/updates and remove every reader; legacy `runnable=1` cannot override V2 denial | Keep rows until PR13. Preserve project/task deletion behavior. |
| Project Milestones/checks/results | Project Agent materializer/routes; readiness, baseline, release readers | Task/WorkUnit lifecycle and Gate | Historical only | Stop writes; remove milestone/readiness dependency from Task/Gate authority | Preserve historical snapshots and checks; do not recompute from current state. Cleanup in PR13. |
| Project Readiness snapshots/inputs | Readiness service and Attention/Project OS views | Current Gate evaluation on exact inputs | Historical only; “ready” cannot make a Task ready-to-merge | Stop snapshot writers and remove admission/readiness reads | Preserve old input snapshots unchanged. Cleanup in PR13. |
| Project Release/references/media pins | Release service and Project OS views | Exact merge admission, Task lifecycle, Gate, ValidationRun, Evidence | Historical only; releases are not recalculated or mutable under new policy | Stop new release creation and remove authority reads; retain release media pin integrity | Preserve records, pins, media IDs, and bytes. Do not run media GC because a release engine was retired. Cleanup in PR13. |
| `project.charter_*`, current Charter pointers, and setup-required fields | V076 compatibility projections and Charter bootstrap | No runtime authority; new Project creation uses normal Project defaults | Display/history only | New Projects are not setup-gated; no service or trigger may read these fields for admission | Keep columns until PR13. Reads can remain during PR12. |

Shared `media_asset`, Task media attachments, media tombstones, `WorkspaceLease`,
WorkUnit isolation, Execution snapshots, Gate evaluation, ValidationRun,
Evidence, Task lifecycle, and the generic OrchestratorRuntime are not retired.
Release media pins remain historical references; PR11 moves or deletes no
bytes.

## Migration strategy

1. Re-audit the actual `origin/main` tree and add a new numbered, additive
   migration after the current migration head. Do not edit historical SQL.
2. Add a small `pr11_vertical_migration_issue` ledger with a unique
   `(source_kind, source_id)` key, a closed disposition set
   (`migrated`, `historical_only`, `already_represented`, `ambiguous`,
   `not_applicable`), a reason, JSON details, and creation time. It is a
   reconciliation receipt, not a general migration framework.
3. In one transaction, retire user/Project auto-bootstrap triggers, fence new
   legacy writes and turn claims, record exact historical dispositions, and
   reconcile only memberships whose prior eligibility depended on the retired
   Project binding. The migration is restart-safe and preserves all source
   rows.
4. Terminate affected active and suspended RoleMemberships with the existing
   lifecycle fields, bump versions, clear a singleton compatibility projection
   only when it names an ended Agent, and persist Task change events in the same
   transaction. Never choose a replacement Actor. Global Agents and
   account-owned Agents whose owner is the Project owner/member retain their
   memberships.
5. Remove Main/Project/Project OS authority readers, Rust materializers, and
   runtime workers. Keep read-only historical access only where PR12 needs it.
6. Keep `OrchestratorRuntime` on generic Task, RoleMembership, Execution,
   Message, Handoff, Proposal, Decision, and domain-event records.
7. Keep Project deletion and WorkspaceLease/media lifecycle contracts intact.
8. V119 narrows V118's update fences for the exact historical FK columns that
   SQLite maintains with `ON DELETE SET NULL`. It permits only a non-null to
   NULL transition after the referenced parent is gone, with every other row
   value unchanged. It also removes V118's cross-field Genesis check that
   prevented handed-off history from surviving Project and handoff deletion.
   The Genesis table rebuild and exact-fence replacement use
   separate commits because SQLite requires `foreign_keys` to be changed
   outside a transaction. The rebuild commit retains deny-all Genesis and
   dependent Charter UPDATE fences and all other V118 retirement fences; the
   later trigger replacement is one transaction. Re-running V119 rebuilds
   Genesis again and converges both from that intermediate state and from a
   completed schema without its `_migration` receipt. Genesis lifecycle and
   source content stay unchanged; its INSERT fence remains active.
9. The existing Project cascade removes handoffs, delivery receipts, messages,
   instruction revisions, and turns tied to its historical Project Chats.
   `ProjectRepo` deletes the immutable leaves while the Project-scoped teardown
   guard and chat provenance still exist; direct leaf deletion remains blocked.
10. V120 keeps TaskRole, Execution role/purpose, WorkspaceLease class, and
    repository capability separate. TaskRole comes from the explicit
    Execution role, or from Task type for `interactive`. Formal Review remains
    the PR8 contract `role=reviewer` and `purpose=review`; interactive-labeled
    Executions use `purpose=general`. Task type independently keeps planning,
    discovery, review, and validation Tasks read-only. `WorkspaceLease.role` is
    only a class: exact reviewer Executions use `reviewer`, and every other
    Execution uses `worker`. A `worker` lease does not mean TaskRole
    `implementer`. Exact Actor, active RoleMembership, repository, and
    capability checks remain required for INSERT and renewal.

Every SQL operation is keyed by exact legacy row IDs or exact Task/role/member
IDs. There is no timestamp, text, latest-record, or current-binding matching.
V118's transaction and unique receipt keys make its membership reconciliation
restart-safe. V119 uses the staged, fenced rebuild described above because
SQLite cannot toggle foreign-key enforcement inside a transaction; no stage
marks V119 complete before its exact-fence transaction commits.

Historical rows are semantically immutable, but mechanical foreign-key
maintenance required by legitimate V2 teardown remains permitted. The
exception applies only to a listed FK column changing from a value to `NULL`
after its parent row is absent; every other column must compare equal. A
semantic update, a direct FK clear while the parent exists, or a mixed FK and
semantic update still aborts. `project_deletion_guard` does not grant this
exception and cannot authorize other legacy writes. Existing `CASCADE`,
`RESTRICT`, and `NO ACTION` relationships retain their outcomes; ProjectRepo
uses its guard and explicit leaf ordering for the historical handoff rows
that otherwise block Project Chat cascades.

The complete V118 fence-to-FK cross-check, with all 152 relations and parent
teardown classifications, is in the [FK fence inventory](plan-pr11-v118-fk-fence-inventory.md).

## Compatibility and rollback

Legacy GET/read paths may continue to expose historical records through PR12.
REST mutations return HTTP 410 `operation_retired`, MCP mutation tools return
numeric error `-32040` with the same code, and database writes/turn claims abort
at the last boundary for direct SQL callers, except for the exact mechanical FK
maintenance described above. Generic Task creation, role changes, Executions,
collaboration, and Project creation remain available through their existing
domain authority.

No temporary synchronization exists, so there is no dual-write divergence to
repair. The additive migration does not erase or rewrite legacy data except for
ending the exact ineligible RoleMemberships that can no longer be authorized.
An application rollback may read the preserved rows, but cannot re-enable old
writers or turn pending jobs back into executable work. Re-enabling the retired
runtime would require a new reviewed architecture and forward migration; PR11
does not drop its fences as a rollback shortcut.

## Data that cannot migrate losslessly

Main-to-Project handoffs and Product Genesis have no Task scope. Charter,
Project Document, Project Decision, baseline, milestone, readiness, release,
AgentAction, commitment, inbox, question, and semantic-memory records do not
generally prove the exact Task, producer Execution/ValidationRun, proposer,
decider, or membership required by the generic model. These remain
`historical_only` or `ambiguous`; matching content or timestamps is not enough.
An old action that already created a real Task is recorded as
`already_represented`; the Task is not duplicated. Existing Tasks referenced
by commitments remain the only Task authority. No synthetic Task, Execution,
HarnessSession, Artifact, Proposal, Decision, Actor, or Handoff is created.

The baseline must report actual ledger counts from the migration fixtures and
the upgraded database. A zero count means no source rows were present; it does
not claim those concepts were migrated.

## Upgrade fixtures

The automated DB migration fixtures use a real V117 schema, then apply V118
and V119.
The row below names the evidence exercised by the PR11 retirement fixture or
the focused API/service test:

| Fixture | Expected result |
| --- | --- |
| A — pre-V071 Room-era history | The fixture starts at V070, upgrades through V071 and V118, then verifies the single Room transcript and source reference remain readable after a second migration-runner restart. |
| B — V071 chats, bindings, messages | Existing Main/Project chats, bindings, message rows, and turn jobs remain historical; new SQL-created users/Projects receive no chat or binding. |
| C — V072 Genesis lifecycle | Discovery, ready-for-Project, handed-off, and cancelled sessions remain unchanged; an attempted continuation fails and no Project is synthesized. The current schema has no separate `proposal` or `failed` lifecycle value. |
| D — V076 Project OS records | Charter/revision, Document, active Baseline/revision/approval, runnable governance projection, Milestone/revision, and ready snapshot stay in their old tables; governance writes are frozen. |
| E — completed Release with media | A historical Release snapshot, media pin, and media asset remain queryable; Task media repository tests also pass on V118. |
| F — binding-only Agent with active/suspended membership | Both memberships end, Task versions advance, exact singleton projections clear, and one deduplicated Task event is recorded without a replacement Actor. |
| G — binding plus independent account/global eligibility | Global and account-owned Project-member Agents retain their memberships. |
| H — multiple roles and multiple members | The same binding-only Agent appears in implementer and reviewer roles; global/member Agents and a Human share a role; only the ineligible Agent memberships end. |
| I — queued/leased/retry-wait Agent Chat turns | Pending records keep their original states; queued claims, expired lease renewal, and restart reclaims are rejected without a fabricated response. |
| J — pending/approved AgentAction | Pending and approved rows stay historical; status mutation and new execution receipts are rejected. |
| K — Task created by old `task.propose` | The existing Task and completed action target remain; the Task count stays one and no generic Proposal, Decision, or Artifact is created. |
| L — Project with no Tasks | The post-cutover SQL-created Project remains valid with no synthetic Task. |
| M — Project created after PR11 | API and SQL creation produce no Main/Project Chat, binding, Genesis, Charter, or governance bootstrap. |
| N — exact Task-scoped old record | A Commitment with an exact `originating_task_id` is recorded as `already_represented`; it does not create or replace a Task. No other old record is converted to generic data. |
| O — ambiguous old record | A Project Document without proven Task/producer provenance remains readable and is recorded `historical_only`; no generic Artifact is fabricated. |
| P — Project teardown with historical rows | `delete_project(...)` succeeds with handed-off Genesis, Charter and approval provenance, Project Agent binding history, Task Execution, Commitment, MemoryItem, Baseline, Milestone, and a durable Project event. Exact FK references become `NULL`; lifecycle/body/status and other history stay unchanged; Project OS rows follow the existing delete contract; no deletion guard remains. |
| Q — exact cleanup and denied writes | V119 permits Task/Execution and self-binding `SET NULL` maintenance, preserves their historical rows, and rejects semantic updates, direct FK clearing while a parent exists, and mixed FK-plus-semantic updates. Tests rerun V119 without its receipt and resume from post-rebuild/pre-fences. V120 covers the explicit and interactive role matrix for TaskRole, lease class, capability, INSERT, renewal, and stale authority denial. |

Focused tests also verify that legacy REST mutations return 410,
Project/Task creation uses ordinary domain authority, Project Agent dispatch
creates explicit TaskRole membership, and V118 media infrastructure remains
usable. The productive Project deletion service also handles real V117 history
after V119. The V118 fixture is transactional and restart-safe. The V119
fixture separately exercises its durable intermediate state and missing-receipt
retry. These tests do not simulate simultaneous Project deletion, concurrent
membership edits, or a live Execution racing the upgrade. Those remain review
checks for the database's single-writer migration boundary and immutable
Execution snapshots.

## Plan boundaries

### PR12

PR12 owns the removal/redesign of REST, MCP, `forge-ctl`, web UI, generated
public types, and public event vocabulary. PR11 only preserves safe historical
reads and makes retired writes fail closed. PR11 does not introduce `/v2`,
feature flags, public aliases, or broad UI redesign.

### PR13

PR13 owns destructive table/column cleanup, old compatibility projections,
physical removal of preserved Main/Project/Project OS rows, and deletion of
historical-only code after all read consumers and upgrade fixtures are gone.
PR11 adds no table drops and does not alter historical migrations.
