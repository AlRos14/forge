# Design — Embedded Main and Project Agents

## Context

The pre-correction implementation established stable account-owned agent identities, immutable profiles, a native Agent Runtime host, protected credentials, scoped LCM/memory, commitments, typed actions, domain events, Attention, Agent detail, and Mission Control. It also replaced legacy Conversations with general-purpose Rooms and modeled Project Agents as versioned memberships with roles and an optional primary Steward.

Live acceptance proved the Task execution path but exposed two problems in the collaboration layer: a Room response job could survive as a silent lease after failure persistence contended with SQLite, and a Worker membership could be marked primary because the database invariant covered only primary Stewards. More importantly, the intended product does not require a collaboration room. It requires a hierarchy of durable one-to-one agent chats:

```text
Account
├── Main Agent ── global chat (also available from bottom-right launcher)
└── Projects
    ├── Project A ── exactly one Project Agent ── project chat ── Tasks ── Workers/reviewers
    ├── Project B ── exactly one Project Agent ── project chat ── Tasks ── Workers/reviewers
    └── …
```

The Main Agent discovers and organizes. A Project Agent manages one Project. Task agents execute and review bounded work. Handoff is explicit publication, not shared implicit context or a multi-agent Room.

## Goals / Non-Goals

### Goals

- One account-level Main Agent and global chat.
- Exactly one active Project Agent and one project chat for every operational Project.
- Direct Forge identity/profile connection without Smith, TUI, or a separate host.
- A left chat switcher plus a persistent bottom-right launcher for the same global chat.
- Explicit, visible, provenance-preserving Main-to-Project handoff.
- A hard tool boundary: Main Agent manages the portfolio, Project Agent manages Tasks only in its Project, Task agents mutate repositories only in Task Workspaces.
- Scope-isolated LCM, semantic memory, commitments, sessions, and context provenance.
- Finite, observable, restart-safe turn processing.
- Data-preserving migration from legacy Conversations and the pre-release Room implementation using new numbered migrations.

### Non-Goals

- Rooms, participants, addressing, responder policies, bounded rounds, arbitrary threads, or recursive agent discussion.
- Multiple persistent Project Agents for one Project.
- Main-Agent Task management or Project-Agent cross-project management.
- Making Task Workers/reviewers persistent Project agents or chat destinations.
- Implicit cross-scope memory, repository access from core chats, or autonomous cross-project Missions.
- A second Agent Runtime host, Smith compatibility layer, TUI dependency, or Nyx runtime dependency.

## Vocabulary

- **Agent Identity** — stable account-owned product identity. It is not authority.
- **Agent Profile** — immutable execution configuration revision for an identity.
- **Main Agent Binding** — the account's single active global assistant binding.
- **Project Agent Binding** — the single active manager binding for one operational Project.
- **Agent Chat** — the singular durable message timeline associated with the Main binding or one Project.
- **Handoff** — an immutable, typed publication from the Main chat into a Project chat.
- **Task agent** — a Worker or reviewer authorized through an existing Task assignment. It is not a Project Agent binding.

`Room`, `participant`, `default responder`, `project_primary`, and `round` are removed from the user-facing and public domain model.

## Ownership Map

| Layer | Owns |
|---|---|
| Forge | identities, profiles, protected credential references, Main/Project bindings, Agent Chats, message/turn admission, handoffs, permissions, memory ACL/authority, commitments, Tasks, Workspaces, validation, domain events, Attention, approvals |
| Forge embedded host | Agent Runtime/provider composition, protected stores, session lifecycle, event mapping, typed Forge tools, and scope-derived workspace policy |
| Agent Runtime | admitted turn execution, context planning/budgeting, LCM projection/compaction, checkpoints, interactions, runtime events, run manifests, replay compatibility |
| `agent-runtime-lcm` | neutral immutable timeline and summary-DAG contracts, CAS invariants, expansion, projection, and convergence |
| Forge React app | binding setup, chat switcher, global launcher, handoff/navigation, Agent detail, Mission Control, provenance, and visible turn state |

## Cardinality and Authority Invariants

1. An account has at most one active Main Agent binding and exactly one global Agent Chat. If no binding is configured, the chat is visible in setup state but cannot admit a model turn.
2. A new operational Project is created atomically with exactly one active Project Agent binding and exactly one Project Agent chat. A Project created without an explicit eligible selection is instead created with its singular chat in `agent_setup_required` state; it is not operational for Project Agent turns and Forge does not fabricate a binding.
3. The database enforces one active Project Agent binding per Project without a role-dependent predicate. `role` and `is_primary` are not part of this binding model.
4. A migrated Project may temporarily be `agent_setup_required` only when a safe single binding cannot be inferred. It cannot admit Project Agent turns until resolved, but existing Project and Task data are preserved.
5. Binding replacement is an optimistic-concurrency mutation. It changes the responder for future turns but does not rewrite chat messages, handoffs, memory, commitments, or historical profile/session provenance.
6. One identity may hold several explicit bindings, including Main plus Project bindings, but each binding receives a different canonical scope, timeline, memory view, session, and tool policy.
7. Connected but unbound identities do not create independent persistent chats. They remain available for later binding or Task assignment.
8. Task Worker/reviewer authority comes only from Task role assignment, claim, workflow, and review policy. It never satisfies a Main/Project binding invariant.

## Persistence Model

Names are normative at the domain level; exact SQL column naming may follow repository conventions.

### `account_main_agent_binding`

- account/owner ID, identity ID, selected profile revision, state, autonomy policy, tool-policy revision, version, timestamps;
- unique active binding per account;
- replacement records immutable history or a replacement link rather than erasing prior attribution.

### `project_agent_binding`

- Project ID, identity ID, selected profile revision, state, autonomy policy, permission ceiling, subscriptions/wake budget, version, timestamps;
- unique active binding per Project, unconditionally independent of any Task role;
- Project creation writes Project + binding + Project chat + domain events in one transaction.

### `agent_chat`

- ID, kind (`account_main` or `project`), owning account, optional Project ID, status, instruction revision, version, timestamps;
- unique `account_main` chat per account and unique project chat per Project;
- the chat is owned by account/Project, not by a replaceable identity, so binding replacement keeps continuity.

### `agent_chat_message`

- immutable message ID and sequence, chat ID, author kind/identity, content status/classification, model/usage/error metadata, correlation/causation, optional source handoff/legacy Conversation/Room IDs, timestamps;
- completed canonical content is append-only; partial deltas are progressive transport state, not canonical messages;
- references never grant authority to their targets.

### `agent_chat_turn_job`

- triggering message, bound responder identity/profile revision, canonical scope, status, attempt count, finite maximum attempts, lease owner/expiry, next-attempt time, error code, response message ID, version, timestamps;
- statuses: `queued`, `leased`, `retry_wait`, `succeeded`, `failed`, `cancelled`;
- at most one live turn for a triggering user/handoff message and at most one active execution lease per chat;
- user-message durability and responder outcome are separate, so a committed user message cannot masquerade as a completed response.

### `agent_handoff`

- ID, source Main chat/message/turn, target Project/chat, author identity, bounded content, typed source references and revisions, status, correlation/causation, target message/turn IDs, timestamps;
- content is immutable and visible in both provenance views;
- private/secret/protected values and inaccessible memory bodies are never copied;
- replay is idempotent and schedules at most one Project Agent turn.

## Canonical Scope Model

| Use | Canonical scope | Authority source | Runtime workspace |
|---|---|---|---|
| Main Agent chat | global Agent Chat | active Main binding + account policy | deny-all filesystem |
| Project Agent chat | Project Agent Chat | active binding for that Project | deny-all filesystem |
| Task Worker | Task | Task assignment, claim, workflow, Project authorization | only that Task Workspace |
| Task reviewer | Task | review assignment and review policy | Task Workspace with review-only capabilities |

Main/Project Agent Chats are the canonical episodic scope. Project summaries offered to the Main Agent are typed account-level projections, not imported Project chat history. Task-native execution remains Task-scoped so retries/session rotation retain Task continuity without merging core chat history.

## Tool and Responsibility Matrix

| Capability | Main Agent | Project Agent | Task Worker/reviewer |
|---|---:|---:|---:|
| Elicit/structure a new idea | yes | within its Project | no |
| Configured web search | yes | policy-dependent, Project-scoped | Task policy only |
| List/create/rename/archive Projects | allowed through account policy/approval | no | no |
| Read bounded portfolio summaries | yes | own Project only | assigned Task only |
| Publish handoff to a Project | yes | receive/acknowledge only | no |
| Read full Project Agent chat | only through explicit authorized user navigation, not model context | own chat | no |
| Create/update/assign/transition Tasks | no | own Project via typed actions and `TaskService` | only assigned Task operations |
| Review/merge/deliver Task work | no | request/coordinate through policy; no direct merge bypass | existing assigned workflow only |
| Repository/filesystem mutation | no | no | Task Workspace only |

Tool descriptors are server-issued from canonical scope. Model output, handoff text, memory, repository text, or caller-provided IDs cannot widen the matrix.

## End-to-End Flows

### Connect and select the Main Agent

```text
user creates AgentIdentity + immutable AgentProfile
  -> protected provider credential is validated
  -> user selects identity/profile as account Main Agent
  -> Forge atomically replaces/creates MainAgentBinding
  -> existing global Agent Chat remains the same timeline
  -> future turns use the new binding and its exact profile provenance
```

### Create and hand off a Project

```text
user discusses idea with Main Agent
  -> Main Agent asks bounded discovery questions and may use configured web search
  -> Main Agent proposes Project metadata + selected connected Project Agent
  -> Forge policy validates and atomically creates Project + binding + project chat
  -> Main Agent proposes a bounded handoff packet
  -> Forge commits handoff + project-chat message + domain events + one turn job
  -> Project Agent responds in its project chat
  -> global chat receives a bounded delivery receipt/link
  -> user switches to the Project Agent chat to continue setup
```

The Project Agent's response is not recursively fed back into the Main Agent model. Any later handoff is another explicit, visible publication.

### Project Agent creates and manages Tasks

```text
user continues in Project Agent chat
  -> Project Agent reads authorized Project state/memory/commitments
  -> Project Agent submits typed Task action
  -> Forge validates bound Project, permission, budget, contract, and dedupe key
  -> TaskService creates/updates authoritative Task
  -> Task Worker/reviewer follows existing assignment/workflow/Workspace path
  -> durable delivery evidence reconciles to Project Agent inbox/commitment
```

The Project Agent core session never receives the Task Workspace and cannot directly edit the repository.

## Turn Admission, Retry, and Failure Visibility

Message admission uses a short transaction: authorize the chat and binding, append the immutable user/handoff message, create exactly one queued turn job, and append matching domain events. Execution occurs outside that transaction.

Claiming uses optimistic versioning and an expiring lease. Successful response commit uses a short transaction that appends one guarded canonical assistant message, links it to the job, marks the job succeeded, and writes events. Failed execution uses a separate short transaction that either moves the job to `retry_wait` with a bounded reason and next attempt or marks it terminally `failed` when the finite budget is exhausted.

If failure persistence itself contends, no process holds a permanent logical lease: the database lease expires, a deterministic reaper recovers it, and total attempts still cannot exceed the stored maximum. Restart preserves queued/retry/terminal state. Idempotency keys prevent duplicate messages or turns after replay.

The UI renders responder state adjacent to the triggering message. It distinguishes queued, running, retrying with attempt/budget, failed with a safe reason and permitted retry action, cancelled, and succeeded. An absent assistant message with a non-success job is never rendered as a normal completed exchange.

## Runtime, LCM, and Context Provenance

Forge continues to embed the exact pinned Agent Runtime revision through its capability-aware backend. The stable timeline key becomes `(AgentIdentity, AgentChat)` for Main/Project chats and `(AgentIdentity, Task)` for Task execution. Binding/profile/session replacement never rewrites a timeline.

Forge authorizes and selects domain fragments; Agent Runtime remains the only native planner for ordering, token accounting, LCM projection/compaction, serialization, and cache identity. Forge persists a context-admission manifest linked to Agent Runtime's `RunManifest`, including source IDs/revisions/reasons and included/summarized/omitted disposition without protected bodies.

CLI Task executors remain valid Task backends. A narrower CLI chat backend may be used only for a safely migrated explicit binding and must advertise its actual limitations; it is not a Room compatibility layer and cannot claim native LCM/checkpoint guarantees.

## Scoped Memory and Commitments

Memory scopes are account, Agent Chat, Project, or Task. ACL filtering precedes body retrieval, FTS matching, snippets, counts, cursor construction, and ranking. Main-chat memory does not imply Project memory; Project Agent chat memory is visible only under that Project/chat authorization. Publishing across scopes creates a new immutable record with source provenance rather than widening an ACL in place.

Final canonical chat messages may be indexed idempotently from committed domain events. Partial output is never indexed. The active chat's raw history is deduplicated against semantic memory already represented by its LCM timeline.

Commitments remain attached to identity and canonical scope. Main commitments cover discovery, Project lifecycle, handoff, and global organization. Project Agent commitments cover its Project and Task outcomes. Evidence is required for completion, and Task delivery reconciliation remains idempotent.

## Migration Plan

Repository policy forbids editing already-numbered migrations. The Room implementation has migrations through `V070`, so this correction begins with `V071` or later.

1. Add Main/Project binding, Agent Chat, message, turn job, and handoff schema plus unconditional uniqueness constraints.
2. Create exactly one global chat per account and one project chat per Project.
3. Migrate legacy Conversation and pre-release Room messages without changing message IDs or bodies. Preserve source Conversation/Room ID, original sequence, author/provenance, model/usage/error data, guarded instruction revision, and resumable backend metadata where valid.
4. When several source threads map to one chat, merge deterministically by original timestamp, source ID, and source sequence. Preserve the original thread boundary as provenance; do not activate old instructions silently.
5. Infer Main/Project bindings only when there is exactly one safe eligible explicit responder (or one valid active Steward for a Project). Ambiguous or invalid cases become explicit setup-required state. Never infer from a primary Worker row.
6. Migrate active Room turn jobs into explicit bounded states. Expired/ambiguous leases become retryable or terminal according to stored attempts and the new finite budget; they cannot remain silently leased.
7. Migrate Room-scoped LCM/memory/context references to the corresponding Agent Chat scope with immutable source links and authorization preserved.
8. Remove Room tables/code only after integrity checks prove message counts, IDs, ordering, bindings, Task history, memory provenance, and protected-data quarantine. Remove all Room public surfaces in the same breaking change.

Rollback is forward-only and data-preserving: a corrective numbered migration may restore derived surfaces, but historical migrations and user records are never rewritten destructively.

## Public Surface

The implementation SHALL keep REST types, generated TypeScript, MCP descriptors, CLI docs, and API docs synchronized. The intended resource shape is:

- singular Main Agent binding: `/api/v1/account/main-agent`;
- singular Project Agent binding: `/api/v1/projects/{project_id}/project-agent`;
- authorized chat switcher/read model: `/api/v1/agent-chats`;
- chat detail/messages/turn state: `/api/v1/agent-chats/{chat_id}` and nested message/turn resources;
- explicit Project handoff: `/api/v1/projects/{project_id}/agent-handoffs`;
- existing identity/profile/session/context/commitment surfaces adjusted to Agent Chat scopes.

Exact request/response names are finalized during implementation and documented together. Caller-supplied identity, chat, Project, or Task IDs are references to authorize, never authority tokens. Retired Room routes, commands, MCP tools, response types, and event names are deleted rather than aliased.

## Web Experience

- The left navigation has a compact chat switcher containing one Global/Main entry and one entry per authorized Project Agent chat, labeled by Project and agent identity. Configured-but-unbound identities remain in Agent management, not in this list.
- A fixed bottom-right launcher opens the same global chat in an overlay from any page. Opening it does not create a thread or duplicate timeline.
- The Main chat emphasizes discovery, web research status, Project creation/organization, bounded portfolio summaries, handoff receipts, and “Continue with Project Agent” navigation.
- A Project chat emphasizes Project setup, decisions, commitments, Task creation/management, delivery outcomes, and scoped context provenance.
- Room creation, participant management, addressing, responder policy, and round controls are removed.
- Responsive acceptance covers 1280, 768, and 375 widths, keyboard/focus operation, screen-reader names, long identifiers, scroll containment, dark mode, loading/empty/error/retry states, and no horizontal overflow.

## Product Genesis Reconciliation

Product Genesis becomes a versioned discovery protocol used by the Main Agent within the existing global chat. Starting Genesis creates a typed discovery interaction/state, not another chat. When ready, the Main Agent proposes Project creation and publishes the approved discovery packet through the normal handoff. The Project Agent then owns Project setup and may create discovery or implementation Tasks according to Project policy. The Main Agent is never granted Task tools by the Genesis prompt.

## Security and Failure Handling

- Credentials, checkpoints, staged responses, interaction secrets, and host authority grants remain in protected stores and never enter chat, memory, manifests, events, logs, or normal APIs.
- All context and tool operations re-authorize canonical scope at use time; references and text cannot grant access.
- Binding revocation stops new turns. In-flight cancellation/continuation follows bounded backend capability without widening another scope.
- Content guards run before canonical persistence and before publication in a handoff or memory record.
- Domain mutation and matching event commit together. Consumers persist cursor/idempotency state and can replay without duplicating turns, handoffs, actions, Attention, or commitment outcomes.

## Verification Strategy

- Migration fixtures: empty account, legacy Conversations, one/many Rooms per owner, ambiguous responders, primary Worker bug row, leased/retrying/failed jobs, protected content, LCM/memory, and existing Task history.
- Database invariants: concurrency races for Main/Project replacement and Project creation; exactly one winner; no role-dependent uniqueness hole.
- Authority tests: Main Task-tool denial, Project cross-project denial, chat-to-Task Workspace denial, Task role scoping, reference/prompt-injection denial, and protected-data redaction.
- Turn tests: atomic admission, one response, lease expiry, persistence contention, finite retry exhaustion, restart recovery, idempotent replay, visible terminal failure, and cancellation.
- Handoff tests: explicit provenance, bounded content, one target turn, no private/global history leak, no authority propagation, retry/replay idempotency, and user navigation.
- Runtime/memory tests: cross-chat isolation, binding/profile/session rotation continuity, deterministic manifests, LCM conformance, publication immutability, and secret exclusion.
- End-to-end browser proof: connect Main and Project identities, create Project through Main, publish handoff, switch chats, create/manage a Todo Task through Project Agent, execute/review/merge through a Task Worker, and inspect evidence at desktop/tablet/mobile sizes.

## Risks / Trade-offs

- Singular chats intentionally remove arbitrary topic/thread separation. Durable LCM, explicit handoffs, commitments, and Project boundaries provide organization; generic threads can be reconsidered only in a separate proposal backed by a concrete need.
- One Project Agent can become a bottleneck. Task delegation and replacement are the scaling mechanisms; multiple persistent Project managers are out of scope.
- Merging several pre-release Rooms into one chat requires deterministic provenance-rich ordering. Exact original thread grouping remains inspectable even though the product no longer exposes Rooms.
- Main-Agent portfolio summaries must stay useful without becoming implicit cross-project access. They are explicit bounded projections with authorization, not raw chat/memory aggregation.
