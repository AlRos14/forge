---
created_at: 2026-08-13T19:04:05Z
updated_at: 2026-08-13T19:04:05Z
---

# Project Charters, Durable Project Documents, and Evidence-Backed Milestones

## Why

Product Genesis can already turn an idea into a Project and publish a bounded Main-to-Project handoff, but the handoff is still a brief carried mainly by chat. It does not yet give the Project Agent a user-approved, revisioned source of truth for what the Project is, which facts are known, what was merely assumed, what success means, or what remains unresolved. That makes long-lived Project Agent behavior vulnerable to chat drift and forces users to restate consequential decisions.

Forge also has authoritative Tasks, validation outcomes, and Task-scoped image/video uploads, but no Project-level release record. Users cannot yet see a stable answer to “what version are we building, what changed, is it accepted, and where is the walkthrough proof?” A dashboard assembled only from current Task counts would be useful progress telemetry, but it would not be durable release truth.

This change extends the singular Main Agent → singular Project Agent model without introducing Rooms or giving either chat agent repository access. The Main Agent conducts bounded, consequential discovery and proposes a Project Charter. The user approves an exact Charter revision before Project creation. The Project Agent receives that revision, expands it into Project-owned documents and Tasks, and maintains evidence-backed milestones whose released snapshots remain immutable.

## User-Visible Flow

1. The user starts Product Genesis in the existing global Main Chat with a rough idea.
2. The Main Agent asks at most two high-information questions per turn, performs bounded web research only when external facts could change the Project, and keeps facts, user decisions, assumptions, hypotheses, and research findings visibly distinct.
3. The Main Agent proposes a working name, one-line vision, scope, non-goals, success measures, constraints, and unresolved research as a versioned Project Charter draft. It may recommend; it does not silently decide on the user's behalf.
4. The user approves an exact Charter revision and the selected Project Agent. Forge consumes the resulting single-use receipt through `CreateProjectFromCharterApproval`, atomically creating the Project/binding/chat and publishing a handoff referencing that exact approved revision and digest; there is no intermediate handoff state.
5. The Project Agent acknowledges the Charter, avoids re-asking settled questions, resolves open assumptions through bounded direct research or discovery Tasks, and maintains revisioned research/product/design/architecture plus the mode-appropriate Delivery Brief or Execution Plan.
6. The Project Agent proposes the smallest mode-appropriate execution baseline. The user approves its exact digest before any repository-capable implementation Task can run; bounded discovery/planning Tasks may run earlier without mutation authority.
7. Within the approved adaptive envelope, the Project Agent creates and manages Tasks through normal Project policy. Task Workers and reviewers retain exclusive repository/workspace authority through scheduler-issued leases.
8. The Project Agent proposes one or more milestones, keeps live progress derived from authoritative Tasks and validation, and requests a standalone readiness evaluation when its acceptance contract has evidence. The immutable `ReadinessSnapshot` records exact evidence attachment IDs/digests and creates no release pins: a ready evaluation moves an unreleased `active` milestone to `ready_for_release`, while a non-ready evaluation leaves it `active` with typed blocker/stale/reconciliation reasons. Readiness for a correction never moves a `released` milestone out of `released`. Multiple milestones may be active; an explicit `primary_milestone_id` identifies the one emphasized in the Overview.
9. The user releases a `ready_for_release` milestone by naming the exact candidate `ReadinessSnapshot` ID and digest. Forge re-authorizes the same inputs and recomputes the exact same digest inside the release transaction; on a match it atomically creates the immutable release manifest `Mxxx-rN` and release-scoped evidence pins, without creating another readiness snapshot.
10. The Project Overview shows the current Charter revision, active milestones and `primary_milestone_id`, derived progress, unresolved decisions/risks, recent changes, evidence gallery, and immutable release history.

## What Changes

- **BREAKING — Product Genesis gains an exact single-use Charter approval boundary.** A Genesis Project-creation action that previously consumed the ready brief SHALL now consume one active approval receipt bound to the Charter content/render digests and selected Project Agent identity/profile/operating-skill revisions. Clients must render the exact approval target and submit its receipt. Normal non-Genesis Project creation may still create `charter_setup_required` Projects under existing policy.
- **Data-preserving media metadata migration.** Existing media asset IDs, Task media IDs, Task routes/URLs, storage keys, and file bytes remain in place. Forge adds Project ownership, scoped attachments, evidence metadata, and release pins without moving or duplicating bytes and without claiming an on-disk layout break. Deleting a Task still removes its Task attachment/URL; a release pin keeps the same bytes available through an authorized Project evidence URL.
- Upgrade Product Genesis with a versioned Main Agent discovery instruction that turns its running outline into a typed, revisioned Project Charter. It preserves the existing two-question limit and explicit “no Project/handoff yet” disclosure.
- Package the discovery/grilling behavior as server-owned `forge.main.project-discovery/v2`, active only during Genesis, and Project planning/orchestration as `forge.project.orchestration/v1`. Profile prompts may shape tone/expertise but cannot override these operating skills or server policy.
- Add immutable `ProjectCharterRevision` content, explicit approval and supersession records, canonical content and rendered-view digests, and a current-approved pointer. Draft edits append revisions rather than overwriting content.
- Add single-use Charter approval receipts with `active`, `consumed`, and `revoked` lifecycle. One `CreateProjectFromCharterApproval` transaction creates the Project, binding, Project Chat, Charter attachment, immutable handoff/message/turn job, events, and Genesis `handed_off` state, then marks the receipt consumed. Failure rolls back all of it; idempotent replay returns the same result.
- Add a versioned Project Agent operating instruction covering startup, least-privilege research, Project Documents, scope changes, Task delegation, decisions, milestone evidence, validation, and authority limits.
- Add Forge-owned Project Documents with typed kinds (`research`, `delivery_brief`, `product_spec`, `design`, `architecture`, `execution_plan`) and immutable revisions. These are durable domain artifacts exposed as editable/renderable documents; they are not arbitrary repository files and do not grant filesystem authority.
- Add an append-only Project Decision Log whose effective `DecisionRecord` states are exactly `active`, `superseded`, and `invalidated`, with a principal and decision class. Draft, proposal, and rejection are separate candidate/editor workflow records and never effective DecisionRecord states; source provenance remains immutable.
- Add Project Milestones with immutable definition revisions in `draft`, `proposed`, `approved`, or `superseded` state and milestone lifecycle exactly `planned`, `active`, `ready_for_release`, `released`, or `cancelled`. Blockers, stale results, and `reconciliation_required` are typed projections/reasons while an unreleased milestone is `active`, not lifecycle aliases. Multiple milestones may be active, with an explicit `primary_milestone_id`, compact default `M001`/`M1 — Deliver outcome`, and immutable `Mxxx-rN` release revisions. Live progress is a derived projection; release is an explicit user-approved immutable manifest.
- Extend the existing media metadata layer so one existing asset may be attached to a Task and to a milestone/release without a duplicate upload. Every existing asset ID, Task media ID, URL, storage key, and file byte remains in place; only Project/attachment/pin metadata is added. Deleting the Task makes its Task URL unavailable; media pinned by a released snapshot remains available through its Project-authorized stable URL.
- Add Project Charter/Document/Milestone APIs, synchronized Rust/TypeScript types, domain events, Project Agent tools/actions, and a Project Overview UI with responsive evidence and release-history views.
- Inject only authorized, revision-addressed artifacts into the Project Agent context manifest. Semantic memory may point to authoritative artifact IDs/revisions but never becomes a competing copy of Project truth.

## Product Decisions in This Proposal

- **The Main Agent recommends; the user approves.** The Main Agent chooses the next discovery question and may recommend name, maturity, scope, and Project Agent. Only the user approves the Charter, Project creation, Project Agent selection, material scope changes, and milestone release.
- **“Grilling” is bounded pressure-testing.** At most two questions appear in one conversational turn. The Main Agent presses only on unknowns that alter scope, architecture, risk, or the definition of done.
- **Small Projects use the same model with fewer artifacts.** `project_mode` is exactly `compact` or `standard`. A compact Charter can be approved once identity, target user/problem, core loop, MVP scope, non-goal, success signal, and material constraint are coherent; standard mode is required when material UX, architecture, data, integration, security/compliance, migration, operations, or irreversible uncertainty exists. Optional Project Documents are created only when the work needs them.
- **Research is hybrid.** Main and Project Agents may use configured, non-authenticated bounded web search for quick external facts. The Project Agent creates a discovery Task for deep comparison, experimentation, repository inspection, authenticated-browser work, or research that needs its own acceptance/evidence trail.
- **The Charter is not silently mutable.** The Project Agent may propose a new Charter revision or explicit change record, but cannot overwrite the approved baseline.
- **Project ownership transfers at handoff.** Later Main Agent context is another explicit supplemental handoff; only the Project Agent classifies it and proposes a Project Charter change. The Main Agent does not resume Project management.
- **Global organization excludes Project-local intent.** The Main Agent may summarize, categorize, or route among Projects using bounded portfolio metadata, but an existing Project's Charter-defined name, identity, scope, and release state remain Project-local approval flows.
- **Milestone labels are presentation, not identity.** Forge uses a monotonic milestone/release sequence as canonical identity and supports user-facing labels such as `v0.1`, `Private beta`, or `Prototype` for software and non-software Projects.
- **Release is explicit.** The Project Agent may request readiness for an `active` milestone; only a successful standalone `ReadinessSnapshot` moves it to `ready_for_release`, and the user approves release by naming that exact snapshot ID/digest. Release creates one immutable manifest plus release-scoped evidence pins atomically; a released milestone remains `released` while correction readiness is evaluated, and corrections append a later `Mxxx-rN` without mutating history.
- **Execution is explicitly gated.** Before any repository-capable Task is runnable, the user must approve one exact execution baseline digest that names the governing Charter/Document revisions, acceptance/evidence matrix, release policy, capability/risk classes, elevated operations, and adaptive envelope. Within that envelope the Project Agent may split, sequence, retry, or substitute Tasks without repeating approval; changes to outcome, acceptance, risk, side effects, release policy, or elevated operations require reconciliation and a new approval.
- **Forge release means frozen Project evidence, not deployment.** The release action records an internal immutable snapshot; it does not merge, tag, deploy, publish externally, or grant repository authority. Those outcomes may be referenced only when existing Task workflows already produced them.
- **Media storage is shared.** Task and milestone attachments point to the same existing Project-authorized media asset. A released snapshot pins its referenced asset metadata so Task deletion cannot erase release proof; evidence availability is `available`, `quarantined`, `redacted`, or `purged`, and a purge makes the release evidence `evidence_unavailable`.

## Impact

- Depends on completed `add-project-agent-federation-2026-08-12` and `add-product-genesis-chat-2026-08-08` behavior.
- Affected capabilities: Product Genesis, Main/Project Agent prompt contracts, handoff/context provenance, scoped memory, Project documents, Project Task orchestration, Task media, Project Overview, events, REST, and generated TypeScript.
- Backend: new migration beginning after `V075`, API types, repositories, an atomic approval-to-Project transaction, services, typed chat actions/tools, context-manifest sources, event projection, and shared media ownership.
- Web: Charter drafting/approval in Main Chat; Project Overview, document status, milestone lifecycle, acceptance/evidence, release history, and chat deep links.
- Documentation: `docs/architecture.md`, `docs/api.md`, `docs/getting-started.md`, `DESIGN.md`, and `CHANGELOG.md` in the implementation change.
- Public surfaces are otherwise additive. The exact Genesis approval/action and release-pinned media retention semantics SHALL be recorded under `Unreleased > Breaking`; the media metadata migration preserves existing IDs, URLs, storage keys, and bytes in place with no claimed on-disk layout break. Task media routes and pre-deletion URLs remain valid, while release-pin/evidence-availability behavior is documented explicitly.

## Non-Goals

- Rooms, participants, arbitrary threads, multiple Project Agents, or recursive agent-to-agent conversations.
- Main Agent Task management, Project Agent portfolio management, or repository/filesystem access for either chat agent.
- Replacing authoritative Tasks with a Markdown checklist or treating a dashboard projection as workflow truth.
- Requiring full product/design/architecture paperwork for every small Project.
- Automatically publishing a milestone because Task counts reached 100%.
- Copying full Main Chat history, hidden memories, credentials, authenticated browser state, or other Projects into a handoff.
- A general-purpose wiki, binary asset manager, or source-control release system.

## Approval Gate

This proposal is Stage 1 only. No application code or migration is included. Implementation begins only after explicit approval of this change, including the default user approval gates for exact Charter creation/supersession, the execution baseline before repository-capable Tasks, and milestone release.
