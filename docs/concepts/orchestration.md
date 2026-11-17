# Orchestration

This page describes the current event-driven Orchestrator and typed Task coordination lanes. The [task operating model](task-operating-model.md) and [delivery fluency](delivery-fluency.md) summarize the full delivery path.


An orchestrator is an Actor admitted under a TaskRole with the `orchestration`
capability. The capability directs active work across Actors; it is not a
super-agent runtime and not a continuous reviewer. The TaskRole name is an
identifier and has no authority meaning by itself.

PR4's generic Message, Handoff, Proposal, and Decision records provide the
initial durable collaboration surface. Their events use the existing
`domain_event` ledger. A Handoff to a Role does not assign a member, and a
Proposal or Decision does not execute the proposed action. PR11 retires
Main/Project Agent and Project OS write and authority paths without
dual-writing generic records. Their historical rows remain readable for
compatibility, while current orchestration uses Task-scoped collaboration,
Roles, Executions, and durable events.

## Implemented subset: PR6

The PR6 runtime claims committed Task-scoped `domain_event` rows and records a
durable wake for the exact active member of an orchestration-capable TaskRole. Broadcast
events are only low-latency hints. TaskRole coordination mode is captured with
the wake and checked again before dispatch; ambiguous or stale targeting fails
closed. `collaborative` may wake every active member, while `partitioned` and
`independent` dispatch only from explicit allocation or addressing evidence.

Task creation, TaskRole policy/mode changes, and canonical membership writes
also append real durable events inside their SQLite mutation transaction.
An empty TaskRole is not eligible; adding or reactivating a member is the
activation event. This includes compatibility writers because the database
triggers observe the canonical rows. EventBus-only Task notifications are not
authority.

V095 performs a one-time current-state reconciliation for existing, non-
terminal Tasks with eligible orchestrator members. It appends typed
`orchestrator.bootstrap_reconciled` events after the V094 cursor high-water
mark, without recreating historical Task events or replaying the old event
archive. Collaborative targets fan out to active members; independent targets
are materialized only when one active member makes the target unambiguous;
partitioned targets require an exact active Actor allocation to an
orchestrator WorkUnit. The stable role/Actor/WorkUnit dedupe key and the normal
domain-event receipt/wake lease path make migration retry and concurrent
consumers idempotent.

An Agent wake creates a fresh Execution under the exact orchestration-capable
TaskRole and uses a read-only Harness Start. PR6 does not infer or resume a previous session. A
Human wake stays durable as pending Human work and creates no HarnessSession.
Generic event-driven outputs remain bounded to Messages, Handoffs, limited WorkUnit creation, Proposals, and exact-report review triage. Typed Task coordination turns separately support work_unit_plan, review_findings, validation_findings, external_feedback, ask_human, request_validation, phase-3 micro_fix, and propose_replan outputs under their exact authorization and source checks. Cross-Agent fallback is applied by the dispatcher, not emitted as a coordinator action.

Execution lifecycle events from an Execution admitted with `orchestration` are
classified separately and never generically wake another orchestrator. Creating a
WorkUnit is an orchestrator action, not a wake signal; completion, readiness,
allocation changes, and explicitly addressed collaboration can wake eligible
members. Unsupported event producers remain deferred to their lifecycle owner
rather than being promoted from EventBus-only signals.

PR9 Gate evaluations and lifecycle transitions append durable domain events.
The Gate Engine consumes relevant committed fact events (review reports,
terminal ValidationRuns, Evidence, Decisions, WorkUnit and integration facts,
role changes, and policy revisions). A satisfied or unsatisfied evaluation is
an input to PR6 orchestration; the orchestrator may direct rework but cannot
write the Gate result or reinterpret its exact inputs.

PR6 reads the TaskRole policy using a small versioned schema documented in
[Roles and memberships](roles.md). `{}` keeps the current PR6 defaults.
Unknown fields and versions fail closed. The wake captures the TaskRole version
and exact JSON separately from the fixed runtime policy digest. Dispatch and
every pending action require the current TaskRole policy and coordination mode
to match that snapshot. A completed action receipt is historical evidence and
remains a no-op on replay even if the TaskRole changes later; pending actions
still fail closed. A TaskRole-change event uses the current mode's targeting
rules and does not turn an ambiguous independent or partitioned role update
into a generic fanout.

V096 also rechecks the exact TaskRole version, policy JSON, coordination mode,
active membership, wake attempt, Execution, and event provenance inside the
same SQLite transaction that writes a typed action's Message, Handoff,
WorkUnit, or Proposal. The `domain_event` insert is the shared guard boundary;
if the snapshot became stale after action reservation, the guard aborts and the
entity row and event roll back together. That deterministic stale-authority
error fails the old wake rather than entering transient retries. V097 completes
the matching `orchestrator_action` receipt from that same domain-event insert,
so effect, event, and receipt commit or roll back together. Replay still uses
`execution_id + action_index`, digest, and stable `result_id`; a completed
receipt skips current TaskRole validation because it cannot create a new effect,
while every pending action revalidates that authority before writing. Human and
ordinary Agent writes without a matching `orchestrator_action.result_id` do not
enter this guard.

## Responsibilities

The following responsibility and wake lists describe the target architecture;
the implemented PR6 subset above is narrower.

An orchestrator may:

* inspect Tasks, Roles, WorkUnits, dependencies, Executions, Artifacts,
  Evidence, usage, Messages, and workspace state;
* create WorkUnits and assign or request work;
* request implementations, investigations, validation, and reviews;
* communicate, hand off, steer where supported, pause, stop, resume, or
  reassign subject to deterministic policy;
* create Proposals, record allowed Decisions, and request a Human decision;
* coordinate other orchestrators.

These are typed domain actions. An orchestrator does not receive arbitrary
database mutation or direct repository write authority. If the same Actor
needs to implement, Forge creates a separate implementer Execution.

## Work boundaries

The orchestrator chooses the work boundary and acceptance context. The
implementation HarnessAdapter owns individual tool calls, reasoning, context
management, and native editing behavior. Orchestrator policy must not become a
file-by-file or command-by-command script.

## Event-driven wakeups

Agent orchestrators are idle between meaningful events. Wake events include
Execution start/completion/failure/stall, WorkUnit completion, dependency
satisfaction, Handoff, addressed Message, Proposal resolution, validation or
review failure, merge conflict, quota issue, and Human input. Log-token
streaming is not a wake event by default; implementations may debounce or
coalesce related events.

Each wake is a new orchestrator Execution. The target architecture permits
resuming a compatible task-scoped HarnessSession; PR6 always uses Start with a
fresh, Execution-bound HarnessSession. The Execution remains independently
auditable.

## Multiple orchestrators

Several orchestrators may occupy one collaborative TaskRole. They communicate
through Messages and resolve consequential disagreement through Proposals and
Decisions. Disruptive operations are policy-controlled so two orchestrators
cannot endlessly undo each other.

## Steering

The core expresses intent to steer an Execution. The adapter reports whether
steering is native, emulated, queued for a later turn, or unsupported. A
successful queue is not reported as native live steering. Stop/resume fallback
requires explicit policy. PR6 does not expose a direct steering operation;
Messages and Handoffs remain durable communication records only.

## Task coordination turns

A Task may authorize a bounded coordination turn through the versioned Task
coordination record. The turn reuses the Orchestrator wake, attempt, Execution,
and provider-Start receipt machinery but has its own JSON output contract: one
`work_unit_plan` Artifact. It is workspace-free and read-only, creates no
WorkUnits, and cannot grant TaskRole, membership, capability, worker, Gate,
review, merge, or TaskLifecycle authority.

The plan is inspectable and immutable. A separate deterministic materialization
validates the exact plan digest, authorization version, team digest, envelope,
Task version, roles, Actors, dependency/parent graph, review independence, and
Artifact inputs before creating WorkUnits in one transaction with an idempotent
receipt. `manual` remains the default. See the
[PR0 contract](../plans/pr0-task-coordination-contract.md).

## Coordinator mediation of review findings

New guided authorizations set `orchestrator_reviews_findings: true`. Previously
stored envelopes without the field retain their prior routing behavior. Worker
result events wake the exact authorized coordinator, including when its saved
Agent profile also fills another role. No provider work starts on Task creation.

The typed response may contain `review_findings`, alongside `actions`. Each item
pins `authorization_version`, `review_report_artifact_id`, `report_digest`,
`disposition`, `rationale`, and `instructions`. `correct` releases the configured
correction route with the coordinator's actionable brief; `recheck` creates a fresh
review WorkUnit for the same immutable result and the disputed findings; `human`
stops automatic correction for that report. Decisions are immutable and replay
safe. A coordinator never rewrites a ReviewReport or supplies approval evidence.
Current team, role policy, wake lease, exact report and authorization are checked
inside the writer transaction. Revoked, stale, paused or terminal work is rejected.

Codex and Claude Code can coordinate without a repository workspace. Codex uses
its native read-only sandbox. Claude uses a native Read/Glob/Grep tool allowlist,
no inherited hooks or MCP servers, and no permission bypass; this is a tool policy,
not an operating-system sandbox. Formal review uses read-only native tools and
explicit pinned evidence rather than another role's private conversation.

Runtime policy version 2 covers these guided-delivery outputs and read-only
harness policies. Pending wakes admitted under an unsupported earlier runtime
policy fail closed and require a fresh explicit turn; completed receipts remain
historical evidence and provider starts are never replayed to migrate a wake.

Task-scoped triage context includes the exact original scope, result Execution,
commit, workspace path and complete pinned delivery Artifact, plus recent
coordinator decisions. The context byte limit applies before Provider Start;
evidence is not silently truncated. WorkUnit-scoped wakes do not receive sibling
review findings or Task-wide triage history.

OpenCode V2 checks `external_directory` before reading an accepted result outside
the isolated runtime. Forge passes only the exact result/review Workspace roots
through `review_read_paths`, and the daemon checks its registered root again.
There is no global external-directory allow rule. Missing, root, or wildcard
paths fail closed. See [OpenCode permissions](https://opencode.ai/docs/permissions).

## Coordinator verification requests and human decisions

Two typed coordinator outputs extend the bounded set (plan:
[coordinator verification and PR loop](../plans/coordinator-verification-and-pr-loop.md)).
They are implemented as pure parsers in `crates/services/src/coordinator_outputs/`
and a durable service in `human_decision_service.rs`. The runtime applies them
only in typed coordination turns (see "Validation findings and resumes" below).

- `request_validation` names one pinned catalog `check_key`, an exact subject
  (`work_unit_result`, or `task_integration` with the Task id and version), an
  optional `baseline` flag and a `reason` (at most 1024 bytes). It carries no
  command, path, commit, workspace or timeout; unknown fields are rejected.
  The check must be pinned by the current authorization envelope
  (`validation.check_not_authorized` otherwise) and a stale subject version is
  rejected (`validation.subject_stale`). Forge resolves the subject and runs the
  deterministic check; the coordinator never executes anything.
- `ask_human` carries a question (at most 1024 bytes), 2 to 6 options
  (`id` slug, `label`, `description`, `consequence`, mandatory `authority_effect`
  of `none`, `scope` or `authority`), an optional recommendation that names one
  option, and at most 16 typed evidence links (`artifact`, `review_report`,
  `validation_run`, `validation_comparison`, `gate_evaluation`, `execution`,
  `external_feedback`, `work_unit`; optional 64-hex digest). At most 4 new
  outputs per turn.

Policy is asymmetric: unlike the legacy actions, the new kinds are permitted only
when listed explicitly in the role's `allowed_actions`. An absent list does not
permit them (`coordination.output_not_permitted`), so existing roles are not widened.

A human decision is a durable `human_decision_request` bound to one exact source
and to the paused coordination request it resumes. Creation validates that the
source and every evidence link belong to the Task (and that digests match where
Forge stores one) and that the raising Execution is the current coordinator Actor
under the exact authorization. Answering requires the expected version and one of
the stored options. The authority needed depends on the chosen option's
`authority_effect`: `none` needs authority to mutate the Task; `scope` or
`authority` needs a Project owner or admin. Project Copilot never answers.
Answering appends `task.human_decision_answered` in the same transaction and then
creates the exact `human_decision_resume` coordination request for the same
authorization, recorded once on the decision. If the authorization was revoked or
changed, the answer stays durable and the resume is deferred rather than
performed under stale authority. A decision whose source went stale is closed as
`superseded` or `cancelled`. Events carry identifiers and states only, never
question text or notes.

### Validation findings and resumes

A terminal catalog check creates a durable `validation_findings` coordination
request when its run is `failed`, `error` or `stale`, or when a baseline comparison
is `regression` or `inconclusive` (`passed`, `fixed`, `both_pass` and
`pre_existing_failure` never wake the coordinator). The request, its optional
Project batch origin (`validation_triage`), the turn-budget charge (`turn_count`,
the same counter `review_findings` uses) and the event are one repository
transaction. Its exact source is the run or comparison id plus the run Evidence
digest; the idempotency key is that source and the authorization version, so the
durable scanner recreates missing requests after a restart without a second
charge. Pause, revocation and restart behave as for `review_findings`: the
completed provider response is retained and replayed after resume.

The coordinator turn context carries the exact run (status, exit code, bounded
stdout/stderr tails and their output digest), the comparison (classification,
baseline status, exit code and output digest) and the check request subject.
`review_findings` context also lists validation runs of the same reviewed
workspace, commit and snapshot.

The turn output is a `schema_version: 2` envelope with exactly one
`validation_findings` decision (`correct`, `recheck` or `human`) and an optional
`outputs` list (at most 4). `correct` creates one correction WorkUnit for the
checked WorkUnit (only for a `work_unit_result` subject) in the triage
transaction and consumes a rework round; `recheck` re-enqueues the pinned check
on the current exact subject (an unchanged subject replays the existing request);
`human` raises a `human_decision_request` (the coordinator's `ask_human`, or a
deterministic two-option question) bound to the triaged request. `request_validation`
outputs enqueue a `coordinator_request` check with the turn's request as
`requested_by_request_id`. `request_validation` and `ask_human` are accepted only
in typed turns because they need a request to anchor them; the event-driven
`actions[]` channel parses and policy-gates them but rejects them with
`coordination.output_not_permitted`.

When a decision is answered, the reconcile scanner calls `ensure_resume_request`
(or the batch variant with origin `human_decision_resume`) for answered decisions
without a resume request. Creation charges the same turn budget and the durable
`task.coordination_requested` event wakes the coordinator. A
`human_decision_resume` turn must return the exact `resumes_request_id`,
`human_decision_id` and `answered_option_id`, the paused kind's key
(`validation_findings`, one `correct` or `recheck` decision) and an empty
`outputs`. Only paused `validation_findings` requests are resumable in this phase.

### External PR feedback triage and same-PR rework

Trusted, current-head feedback ingested for an open remote PR admission (frozen
policy `ingest = true`) becomes one `external_feedback` coordination request.
The request names the admission as its exact source and the digest of the exact
ordered set (at most 20 items) as its source digest; the set rows
(`task_external_feedback_set_item`), the request, its optional Project batch
origin (`external_feedback_triage`, the only origin accepted while the Task is
`Merging`), the turn-budget charge and the event are one repository transaction.
The idempotency key is the authorization version, the admission and the set
digest, so the durable scanner recreates missing requests after a restart
without a second charge. At most one request per admission is in flight; items a
request consumed are never offered again (a `failed` request leaves them for a
Human). Pause, revocation and restart behave as for `validation_findings`.
While the Task is `Merging`, V178 admits a Running coordinator Execution only
with `work_unit_id = NULL` and an `external_feedback` request for that Task in
`requested`, `running` or `uncertain`; the request's `execution_id` must be null
or match the Execution.

The turn context carries each item (kind, author trust class, path and line,
head state, body bounded to 2 KiB and flagged as untrusted third-party data), the
pull request number, URL and head, and the facts about the Task's original scope
(title, description, WorkUnit scopes, envelope limits, team). The turn output is a
`schema_version: 2` envelope with exactly one disposition per item
(`extend_task`, `propose_task`, `human` or `dismiss`), an optional
`work_unit_plan` (required exactly when an `extend_task` exists) and one
`ask_human` per `human` item. Forge, not the coordinator, classifies scope
(`classify_rework_scope`): an `extend_task` classified `scope_change` fails
validation with `pr_feedback.extend_out_of_scope`. The objective baseline is the
Project batch authorization snapshot of the Task title and description; a Task
outside a batch has no frozen baseline, so that single fact holds vacuously.

After the per-item rows (V173 triage, V175 disposition) and the `triaged` receipt
commit, the effects run idempotently and are retried by durable scanners:

- `extend_task`: `PrReworkService::supersede_for_rework` pauses the admission
  (TaskMerge `abandoned`, `Merging -> Blocked(pr_feedback_rework)`; the PR is not
  touched) and `resume_after_supersession` returns the Task to `Active`. The
  pending plan is the typed output of the completed coordinator Execution; it is
  re-derived and materialized as WorkUnits with deterministic ids, `external`
  provenance `external_feedback:{request_id}` and the exact item ids and body
  digests appended to their instructions. Plan items with `review_subject_key` or
  `artifact_inputs` are refused at validation. After the WorkUnits exist the Task
  follows the normal implement, review, Gate and TaskMerge path.
- `propose_task` and `human`: a `human_decision_request` (source `external_feedback`,
  `source_id` the item id) is raised and bound to the disposition. Approving a
  `propose_task` decision (`authority_effect = scope`) creates the dependent
  follow-up Task through `FollowUpTaskService`; the current PR is unchanged. These
  decisions never create a `human_decision_resume` request (the answer reports the
  deferred code `human_decision.direct_effect`), and a `human` answer is recorded
  without an automatic coordinator turn.
- `dismiss`: the immutable disposition (with its reason) is the receipt.
- A remote merge observed during rework raises a `rework:{id}` decision; when it
  is answered the scanner calls `converge_remote_merged`, which cancels the
  unstarted rework WorkUnits, abandons the pause and blocks the Task with
  `pr_merged_externally`.

The external action service supports post_comment for a deterministic feedback summary Artifact. It creates a durable action under the admission's frozen policy; the action is Human-approved by default, automatic only when the repository policy explicitly permits it, and off when post_summary=off. The service checks the anti-loop marker before posting and reconciles uncertain outcomes.


## Preauthorized fallback, micro-fix, and replan

Coordination envelope v2 may preauthorize up to three fallback Agents per TaskRole, a bounded micro-fix route and formal reviewer, and coordinator replanning. The dispatcher considers fallback only after an eligible executor_unavailable route is exhausted. A future retry within the configured wait bound retries the same Agent; an uncertain Provider Start never triggers fallback. Substitution keeps the same WorkUnit, TaskRole, scope, and Workspace, and writes an immutable receipt.

A micro-fix must cite findings and paths from its exact source report. The diff is measured from the reviewed commit and is limited to three files, sixty changed lines, and at most three micro-fixes per Task; undeclared paths, dependency changes, and protected control files fail the bounds check. Every result goes through an independent formal review and a fresh Gate.

A coordinator replan is classified in Rust against the objective, current team, existing and proposed WorkUnits, budget, repository/path scope, and envelope grants. At most two replans are recorded per Task. If classification detects a scope or authority change, an exact Human decision is required before that plan can be materialized. See [delivery fluency](delivery-fluency.md) and the [coordinator verification and PR loop plan](../plans/coordinator-verification-and-pr-loop.md).
