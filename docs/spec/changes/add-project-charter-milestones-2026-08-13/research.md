# Research Notes

## Sources Consulted

- Forge's implemented singular Agent Chat, Product Genesis, scoped memory/context-manifest, Task workflow, and Task media contracts.
- `../app-skills` (Spark), especially its `/start` conductor, `mvp-grill`, docs-first workspace, approval gate, planner/implementer/evaluator separation, and task-status projection.
- `../symphony`, especially its persistent Workpad, milestone update discipline, targeted validation proof, and image/video walkthrough handoff.
- A signed-in ChatGPT Pro design review driven through `ap-browser`, asked to pressure-test the role contracts, handoff, revision model, milestones, evidence, least-privilege research, migration, and acceptance scenarios.

## Adopted from Spark

- Ask only questions whose answers can change scope, architecture, risk, or “done.”
- Make the discovery output durable before execution and place an explicit approval boundary between proposal and Project work.
- Keep product truth in revisioned artifacts rather than expecting chat history to act like a specification.
- Separate planning/coordination from Task implementation and independent validation.
- Provide a fast path for a coherent small Project and a deeper path only when uncertainty warrants it.

## Adapted Rather Than Copied from Spark

- Spark permits up to five questions in a round; Forge keeps its existing limit of two questions per conversational turn to suit a long-lived chat interface.
- Spark's artifacts are repository Markdown files. Forge Main/Project Chats intentionally have deny-all filesystem policy, so Forge owns the artifact records and exposes render/export views without granting repository access.
- Spark's `tasks.md` is workflow truth in its local scaffold. Forge already has authoritative Task rows and state-machine validation, so document checklists may summarize or link Tasks but cannot replace them.

## Adopted from Symphony

- Maintain a compact, persistent status projection and update it after meaningful changes: approved scope, research resolution, Task outcome, acceptance result, and release.
- Attach targeted proof to the work it validates, with image/video walkthroughs for user-visible behavior.
- Treat proof as part of handoff/release readiness rather than decorative media.

## Adapted Rather Than Copied from Symphony

- Symphony's Workpad is one issue comment. Forge should derive its Project Overview from typed Charter, Document, Decision, Task, validation, Milestone, and Evidence records so status cannot drift from its authorities.
- Symphony delegates media upload to a separate skill. Forge already owns Task media storage; this change generalizes associations and stable Project authorization instead of adding a second binary store.
- Symphony status labels are issue-orchestrator states. Forge Milestones are user-facing release contracts above the existing Task workflow, not a replacement state machine.

## Pro Review Conclusions Incorporated

- The important boundary is not merely a better prompt: Charter content, approval, provenance, digest, and supersession must be server-validated domain state.
- Milestone evidence must remain valid after Task cleanup or soft deletion. A release therefore pins shared Project media assets and references stable Project URLs rather than relying only on Task-lifecycle URLs.
- Main-Agent naming and scope synthesis should be recommendations with visible rationale; user approval is authoritative.
- Project Agent research should use a least-privilege hybrid: quick bounded search in chat, durable discovery Tasks for work needing depth, experiments, files, authenticated state, or independent evidence.
- Released snapshots must freeze exact inputs—document revisions, decision set, Task/validation state, git references, and evidence—not merely store a mutable milestone title and percent complete.

## Pro Adversarial Findings and Resulting Decisions

The signed-in ChatGPT Pro review was used as an adversarial contract pass, not as approval or implementation authority. It identified the following failure modes; each decision below is now normative in this change:

| Adversarial finding | Resulting decision |
|---|---|
| A `handoff_pending` state could expose a Project or handoff without the other half committed, and a retry could duplicate it. | Do not add that state. Genesis has exactly `discovering`, `ready_for_project`, `handed_off`, and `cancelled`; `CreateProjectFromCharterApproval(approval_id, idempotency_key)` consumes one exact active receipt and atomically creates Project, binding, chat, Charter attachment, handoff, turn job, events, and `handed_off`. |
| A ready brief or a model message could be mistaken for approval, or a receipt could be replayed against a newer draft. | Use an immutable, principal-bound, single-use receipt over the exact Charter content/render digests and selected Agent identity/Profile/skill/policy revisions; silence and model output never approve, and replay returns the original result. |
| A single “fast/slow” or arbitrary mode can silently omit material risk review. | `project_mode` is exactly `compact` or `standard`; compact uses the smallest sufficient Delivery Brief, while standard addresses material UX, data, integration, security, migration, operations, or irreversible uncertainty. |
| A Charter could be treated as sufficient authority for implementation, or the Project Agent could keep asking for approval for every safe split. | A user-approved, digest-bound execution baseline is required before repository-capable Tasks run. It carries an adaptive envelope for safe split/sequence/retry/substitution; changes to outcome, acceptance, risk, side effect, release policy, or elevated operation require reconciliation and a new approval. |
| Material scope changes could be hidden as document edits, leaving old Tasks and checks falsely valid. | Use a typed `CharterAmendment` with base/candidate revisions, material diff, rationale, and affected records. On approval, attach a typed `reconciliation_required` projection/reason to affected Decisions, Documents, Tasks, baselines, validations, and milestones until each is retained, revised/replaced, cancelled, invalidated, or superseded explicitly; it never becomes an effective DecisionRecord state. |
| A global “latest record wins” hierarchy can let a stale Document or dashboard override a Charter, baseline, validation, or release in another domain. | Compute domain-specific typed `EffectiveProjectState`, naming each authority source and event watermark; record canonical conflicts and block only affected paths. Status, chat, memory, and dashboards remain projections. |
| Agent identity alone is insufficient to make approvals, checks, waivers, or releases trustworthy, and a Project Agent or reviewer could assess its own work. | Bind every approval, validation, manual check, waiver, and release to principal, authorization basis, exact target/input digest, governing revisions, expected version, explicit event, timestamp, and idempotency key. Workers submit but do not validate their own work; reviewers are independent; the Project Agent may propose but never self-review, self-attest, self-waive, or self-release. |
| A readiness digest computed before release can become stale in the release race, and release-side pinning can accidentally create a second candidate record. | A standalone readiness evaluation persists one immutable `ReadinessSnapshot` that references exact evidence attachments/digests and creates no pins. A release request names that exact snapshot ID/digest; the release transaction re-authorizes the same inputs, recomputes the exact same digest, and on a match atomically creates the release manifest plus release-scoped evidence pins. A mismatch creates no release or pins and no new readiness snapshot. |
| A mutable project default or UI label could change release gates after baseline approval, and a thin readiness row could omit the exact context used to compute it. | Freeze release policy, reviewer-independence, evidence-availability, freshness, and allocation rules inside the approved execution baseline. Persist the immutable `ReadinessSnapshot` with ordered input manifest, event watermark, policy/context refs, `ready|blocked|failed|stale` result, exact attachment/digest references, and readiness digest. A ready evaluation moves unreleased `active` to `ready_for_release`; a non-ready result leaves it `active` with typed blocker/stale/reconciliation reasons, and correction readiness leaves `released` unchanged. |
| A singular “active milestone” hides parallel outcomes, and free-form labels make release identity mutable. | Permit multiple active milestones and persist explicit `primary_milestone_id`. Use immutable Project-local milestone IDs `M001`, `M002`, … and immutable per-milestone release revisions `M001-r1`, `M001-r2`, …; labels remain presentation only. |
| A single state list can conflate milestone definition revisions, milestone lifecycle, and diagnostic projections. | Keep definition-revision states exactly `draft|proposed|approved|superseded`; keep milestone lifecycle exactly `planned|active|ready_for_release|released|cancelled`. Blocked, stale, and `reconciliation_required` are typed projections/reasons while active, never lifecycle aliases. `ReadinessSnapshot` is the immutable release-candidate record; no separate release-candidate lifecycle state exists. |
| An effective decision can be mistaken for an editor proposal, approval workflow, or rejection row. | Keep effective `DecisionRecord` states exactly `active|superseded|invalidated`, and store principal plus decision class. Draft/proposal/rejection remain separate candidate/editor workflow records and cannot enter the effective DecisionRecord state set. |
| Media can appear valid after quarantine/redaction/purge, or a purge can silently rewrite historic proof. | Evidence availability is `available`, `quarantined`, `redacted`, or `purged`. An authorized redaction or mandatory purge preserves permitted tombstone/digest/audit metadata and marks affected release evidence `evidence_unavailable`; historic release content is never rewritten. |
| Passing a filesystem path, Workspace token, or repository handle through Task prose lets chat agents bypass scheduler isolation. | Tasks accept only a logical `repository_binding_id`; the scheduler alone issues a short-lived Project/Task/base-ref/role/capability/principal-bound `WorkspaceLease`, which is never exposed to Main or Project Agent context. |
| “Migrating” media by moving files or introducing a second blob store could break old references and duplicate bytes. | Preserve every existing asset ID, Task media ID, URL, storage key, metadata, and file byte in place. A new internal Project `MediaAsset` identity may map to the legacy row but never replaces it. Add only Project/attachment/evidence/pin metadata; do not move or duplicate bytes and make no on-disk layout-break claim. |
| Auto-adopting old Projects from chat/Tasks would fabricate user decisions and create false release authority. | Legacy Projects remain `legacy_unverified`/`charter_setup_required`; adoption drafts are unapproved and can block release only. Existing chat, Tasks, evidence, and Documents remain usable until the user explicitly approves an exact adoption Charter. |

## Rejected Alternatives

- **Let the Main Agent create and manage initial Tasks.** This erases the product responsibility boundary and lets the global scope mutate Project work.
- **Send a long prose handoff and rely on memory.** Prose is hard to validate, version, approve, diff, or load deterministically after compaction.
- **Give Project Agent a repository workspace so it can write docs.** Project Documents are product state; granting filesystem access would also create an unsafe path to implementation outside Task policy.
- **Store milestone videos in a second folder/table unrelated to Task media.** This duplicates upload validation, authorization, retention, serving, and cleanup behavior.
- **Compute releases automatically from Task completion.** Task completion does not prove the agreed outcome, and released truth must not change when later Task records move or are deleted.

## Compatibility Assessment

- Replacing a ready Genesis brief with mandatory approval of an exact Charter revision/digest changes an already shipped beta behavior. The proposal therefore treats the Genesis creation/action contract as a visible breaking change, updates every caller together, and requires an `Unreleased > Breaking` changelog entry rather than a compatibility alias.
- Existing non-Genesis Projects, Agent Chats, Tasks, and Task media remain data-preserved. They are not assigned fabricated approvals; they use the explicit adoption path only when they need a current Charter/release.
- Shared media metadata preserves every existing asset/Task ID, route/URL, authorization behavior, storage key, and file byte in place. Release pins change evidence retention semantics, but the migration moves or duplicates no bytes and makes no on-disk layout-break claim.
