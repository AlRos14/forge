---
created_at: 2026-08-12T19:22:52Z
updated_at: 2026-08-13T03:35:37Z
---

## Revision Note — Direct Agent Chats, Not Rooms

The first implementation and live browser acceptance established a useful baseline, but the product model was corrected on 2026-08-13: Forge does not need general-purpose Rooms, participants, addressing, or multi-agent rounds. The intended experience is one global Main Agent plus exactly one Project Agent for each operational Project. The user switches among those agents' durable chats from the left navigation, while the global chat is also always reachable from a bottom-right launcher.

This revision preserves the implemented identity/profile split, direct embedded Agent Runtime host, scoped LCM and memory, commitments, typed actions, Task execution, domain events, Attention, and Mission Control foundations. It replaces the Room collaboration layer with singular agent chats and explicit Main-Agent-to-Project-Agent handoffs. Because this is a material product and public-API correction, the change returns to proposal approval before implementation continues.

## Why

Forge already schedules and verifies Task-scoped coding agents. It now also has the foundations for durable embedded identities that can connect directly to a provider/model and retain scoped continuity. What it still needs is a clear product hierarchy:

- the Main Agent helps the user shape ideas, search the web, create and organize Projects, and hand context to the correct Project Agent;
- each Project has one persistent Project Agent that completes setup and creates/manages that Project's Tasks;
- Task Workers and reviewers continue to execute bounded work through the existing Task workflow and are not additional Project Agents;
- the user can switch directly between the global chat and Project Agent chats without creating or administering Rooms.

The general Room abstraction obscured those responsibilities and created unnecessary participant/routing/round state. Live acceptance also showed that a Room turn could remain silently leased after response persistence failed, and that the former partial primary-Steward constraint permitted a primary Worker. Singular bindings and explicit turn state remove both ambiguity classes.

## Source Baseline

- Product input: `/Volumes/Data/Downloads/forge-v2-project-agent-federation-compact.zip`, recorded with hashes in `source-baseline.md`.
- Product correction: the user-directed one-Main-Agent/one-Project-Agent chat model captured in this revision.
- Runtime input: Codex session `019ff28b-1649-74f2-946c-4e0ede350789` and Agent Runtime revision `a7075b1d2dd1cee05db63bc480ff46b0f97ec239`.
- Memory donor: Nyx revision `9614842d8f614d7d41e00d8e73ed3d042764d451`; Nyx remains a design donor, not a Forge dependency.
- Host evaluation: `../tui` remains a composition reference only. Forge embeds Agent Runtime directly and does not require Smith or TUI.
- Delivery dependency: `docs/spec/forge-v2-autonomous-work-management/` remains authoritative for Task, Run, Workspace, validation, review, and delivery behavior.
- Acceptance evidence: `test/live-agent-20260813-0057/ACCEPTANCE.md` and its browser proof capture the exact pre-correction baseline.

## What Changes

- **BREAKING — replace Rooms with singular Agent Chats.** Remove account/project Room creation, participants, addressing, responder policies, and bounded rounds from REST, MCP, CLI, web, and domain terminology. Each account has one global Main Agent chat; each operational Project has one Project Agent chat. Existing Conversation and pre-release Room messages are migrated data-preservingly into these chats with immutable origin provenance.
- **BREAKING — replace Project memberships/primary roles with one Project Agent binding.** An operational Project has exactly one active `ProjectAgentBinding`. There is no role/`is_primary` combination and therefore no state in which a Worker can become the Project's primary agent. Task Worker/reviewer authority remains on Task assignments.
- Add one account-level `MainAgentBinding`. A connected identity may exist without either binding, but it appears in the chat switcher only when it is the Main Agent or a Project Agent.
- New Project creation binds its Project Agent atomically. A migrated Project for which no single safe binding can be inferred enters explicit `agent_setup_required` state; its existing data and Task workflows remain readable/usable, but Project Agent turns are unavailable until the user selects one identity.
- Add explicit, immutable Main-to-Project handoff records. A handoff publishes a bounded, provenance-linked context packet into the target Project chat and schedules at most one Project Agent turn. It never copies credentials, private memory, hidden global history, or Main Agent authority.
- Enforce responsibility boundaries in server-issued tool policy. The Main Agent may elicit requirements, use configured web search, create/list/organize Projects, inspect bounded portfolio summaries, and hand off. It cannot create, edit, assign, transition, review, merge, or otherwise manage Tasks. A Project Agent may create and manage Tasks only in its bound Project through existing Task services and policy envelopes; its core chat receives no repository workspace.
- Keep direct embedded-agent connection in Forge. Stable `AgentIdentity` and immutable `AgentProfile` revisions remain account-owned; a single identity may serve multiple explicitly bound scopes, but each chat/Task receives a separate canonical context and authority grant.
- Bind one logical LCM timeline to each Main chat, Project chat, and Task execution scope. Forge keeps scoped semantic memory, commitments, context provenance, protected runtime state, and Agent Runtime as the single native context/token-planning authority.
- Make chat turn outcome explicit and finite. User messages are immutable ledger entries; a separate turn job exposes queued, leased, retry-wait, succeeded, failed, or cancelled state. Retry budgets are finite, leases expire, failure persistence is idempotent, and the UI never represents a missing assistant response as silent success.
- Add the chat switcher to the left navigation and a persistent bottom-right global-chat launcher. Both entry points open the same global timeline. Project chats show their Project identity/context and Task-management affordances; the global chat shows portfolio/discovery/handoff affordances. Room administration UI is removed.
- Refocus Mission Control on the Main Agent, one agent per Project, relevant Task Workers/reviewers, Attention, work, commitments, and outcomes instead of treating every configured profile as a persistent conversational agent.
- Rebase Product Genesis onto the global Main chat and explicit Project handoff. It must not create a second conversation/chat or bypass the Main/Project tool boundary.

## Important Reconciliations

- The user-facing term is **Agent Chat**. `Room` is not retained as an alias, route, type, command, or navigation concept.
- Exactly one Project Agent means one persistent manager for each operational Project. It does not prohibit multiple Task Workers/reviewers assigned through existing Task roles.
- The same identity may be Main Agent and/or Project Agent for multiple Projects only through separate explicit bindings. Its timelines, memory views, tools, commitments, and workspace authority remain scope-isolated.
- The Main Agent may read bounded Project portfolio summaries needed for global organization, but it cannot mutate Project Tasks or read private Project-chat history implicitly. A typed Project reference or explicit handoff is not an authority grant.
- The Project Agent proposes and manages work through `TaskService`; implementation still occurs only in an assigned Task Workspace under workflow, validation, review, and delivery rules.
- No feature flag or compatibility shim is introduced. Since migrations `V059`–`V070` have already been executed in local acceptance data, the correction uses new numbered migration(s) beginning at `V071`; historical migration files are not edited.
- The current pre-release implementation is a tested baseline, not shipping approval. Browser blockers and the product correction keep release gate 10.7 open.

## Impact

- Affected specs: `embedded-agents`, `project-agent-federation`, `agent-chats` (replacing `project-rooms`), `scoped-agent-memory`, `persistent-agent-runtime`, `agent-commitments`, `durable-attention`, and `mission-control`.
- Affected backend: new numbered migrations; identity/binding/chat/message/turn/handoff repositories and services; runtime/session scope mapping; Task proposal policy; event consumers; App/MCP state composition.
- Affected public surfaces: Room REST/MCP/CLI/types/events are removed and Agent Chat/Main Agent/Project Agent/handoff surfaces replace them. Generated TypeScript and docs change in the same implementation.
- Affected web: navigation, global launcher, chat switcher, Main and Project chat experiences, identity binding, handoff state, explicit turn failures, Agent detail, and Mission Control.
- Documentation: `docs/architecture.md`, `docs/api.md`, `docs/getting-started.md`, `docs/cli.md`, README links if necessary, and `CHANGELOG.md` with visible `### Breaking` entries.

## Non-Goals

- General-purpose Rooms, arbitrary chat threads, participant lists, `@` addressing, multi-agent rounds, or recursive agent debate.
- More than one persistent Project Agent per Project.
- Letting the Main Agent manage Tasks or letting a Project Agent manage another Project.
- Treating Task Workers/reviewers as persistent Project Agents or chat-switcher entries.
- An omniscient global assistant, implicit cross-project memory, or implicit Project-chat access.
- Repository mutation from Main/Project chat sessions; all code mutation stays in Task Workspaces.
- Autonomous cross-project Missions, automatic merge authority, or a replacement workflow engine.
- Smith compatibility support, TUI embedding, Nyx runtime dependency, or committed sibling filesystem paths.

## Assumptions Resolved by This Proposal

- There is one Main Agent binding and one global chat per account. If an account has not selected a Main Agent, Forge presents explicit setup state rather than fabricating a responder.
- New Projects are created with exactly one Project Agent binding. Imported ambiguous Projects use `agent_setup_required` until the user resolves the identity; existing Task execution is not destroyed or silently reassigned.
- A handoff may be repeated as a visible context update. Every publication is attributed and immutable; no implicit continuous memory bridge exists.
- Opening the global launcher while viewing a Project does not automatically expose Project-private context. The user may attach a typed Project reference, after which only authorized bounded summary data is available.
- Connected but unbound identities remain usable for later Main/Project binding or Task Worker/reviewer assignment, but they do not create extra durable chat threads.

## Approval Gate

This revision is Stage 1. No further implementation or migration work begins until the revised proposal, design, task plan, and delta specs are explicitly approved. Approval authorizes replacing the pre-release Room surface with the singular Agent Chat model; it does not authorize shipping until all revised validation and browser gates pass.
