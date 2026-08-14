# Source Patterns and Adoption Boundaries

These adjacent repositories informed this skill. Reuse the named mechanism; do not inherit unrelated product vocabulary, authority, storage, or UI wholesale. Reinspect the cited source before implementation because adjacent repositories can evolve independently.

## Forge: Product Authority and Integration Host

Forge remains the source of truth for:

- the singular global Agent Chat and singular Project Agent Chat per Project;
- authenticated Agent bindings and server-owned operating instructions;
- Product Genesis, Project creation, Tasks, Task workflow/review, events, storage, and REST/UI conventions;
- scoped context-manifest and memory publication;
- Task media validation, authorization, stable URLs, and cleanup behavior.

Read the repository `README.md`, `docs/architecture.md`, `docs/api.md`, current migrations, and affected code before implementation. The approved change under `docs/spec/changes/add-project-charter-milestones-2026-08-13/` is the implementation contract when the user approves Stage 2.

Do not reintroduce the removed Room or participant model. Main is global; each Project has one Project Agent; Workers/reviewers remain Task-scoped execution principals.

## Spark / app-skills: Grill, Durable Product Truth, and Execution Gate

Relevant sources:

- `../app-skills/templates/vite-react/docs/spark/project.md`
- `../app-skills/templates/nextjs/docs/spark/project.md`
- `../app-skills/docs/spec/project.md`
- the start/plan/implement/evaluate skills and templates applicable to the selected Spark pack

Adopt:

- a short, explicit north star containing vision, target user, core loop, and non-goals;
- interviewing the user instead of inventing a missing consequential answer;
- “chat steers; durable workspace state decides”;
- derived status instead of a separately editable completion field;
- a visible planning-to-execution approval boundary;
- planner / implementer / independent evaluator separation;
- artifacts that exist to control delivery, not to maximize paperwork.

Translate into Forge:

- the Charter is the mandatory Product north star;
- compact mode uses a Delivery Brief; standard mode selects applicable product/design/architecture artifacts;
- one approved execution baseline replaces a per-file or per-Task execution gate;
- Forge typed records and events—not repository Markdown—are canonical;
- Workers/reviewers—not the Project Agent—receive repository Workspaces.

Do not copy Spark's filesystem source-of-truth model, stack defaults, pack registry, status names, or mirrored-skill build mechanics into Forge without a separate approved reason.

## Agent Runtime / Nyx LCM: Lossless Episodic Context

Relevant source:

- `../agent-runtime/docs/spec/changes/add-lossless-context-memory-2026-08-11/`
- user-provided prior work reference `019ff28b-1649-74f2-946c-4e0ede350789`

Adopt the host-neutral mechanism where Forge needs long-running conversation continuity:

- immutable ordered entries;
- derived summary DAG with supersession rather than destructive transcript replacement;
- stable authorized expansion pointers back to original entries;
- deterministic provenance, classifications, revisions, and content fingerprints;
- compare-and-swap/idempotent mutation and checkpoint recovery;
- context planning as the only provider-visible ordering/budget authority;
- separation of episodic LCM from semantic/project memory.

Forge owns the product binding and ACL. Bind timelines to authenticated Main or Project Agent context; a timeline ID never grants access. Preserve the runtime's protected checkpoints and raw history, but exclude those protected bodies from Charter handoffs and ordinary model context.

Do not copy outdated consumer examples literally: adjacent Agent Runtime design may mention a Forge Room. This skill's current product contract intentionally has no Room.

## TUI / Smith Memory: Durable Host-Owned Project Memory

Relevant sources:

- `../tui/docs/spec/changes/add-file-backed-project-memory-2026-08-02/`
- `../tui/crates/smith-runtime/src/memory.rs`
- `../tui/crates/smith-runtime/src/project_instructions.rs`
- `../tui/crates/smith-runtime/src/chatgpt.rs` and provider/auth composition only when Forge embeds or connects that runtime

Adopt:

- host-owned, Project-private memory with explicit policy and revisioned context contribution;
- memory as a bounded retrieval aid, never canonical history or authority;
- exact Project binding and sensitivity-aware publication;
- atomic mutation, path/symlink safety where file-backed storage is used, deterministic repair, and context-boundary snapshots;
- current source verification when a remembered claim can become stale.

Forge already has scoped memory/context-manifest work. Reuse the neutral Agent Runtime seam and Forge's database/authorization model instead of adding a Smith-only dependency or a second file store by default. A direct embedded agent can use Smith/TUI provider-auth composition only behind Forge AgentIdentity/Profile/binding policy; it does not require “Smith support” as a public Forge domain.

## Symphony: Milestone Workpad and Proof Media

Relevant sources:

- `../symphony/README.md`
- `../symphony/elixir/WORKFLOW.md`
- `../symphony/SPEC.md`

Adopt:

- update durable state at meaningful delivery milestones;
- validate against explicit acceptance criteria before handoff;
- prefer targeted proof that directly demonstrates the changed behavior;
- record commands/results and capture walkthrough images/video for UI/runtime changes;
- preserve an inspectable work/review trail.

Translate into Forge:

- Task events, validation attestations, Decisions, and milestone evidence replace a mutable issue workpad as canonical state;
- `$forge-proof-media` captures/uploads/comments UI proof through existing Forge capabilities;
- Project-owned media assets can have Task/milestone attachments and immutable release pins without duplicating bytes;
- proof is relevant only when pinned to acceptance check plus Task/run/commit/build context and an allowed principal.

Do not inherit Symphony's Linear/GitHub-specific lifecycle, automatic landing behavior, or assumption that a walkthrough alone proves release readiness.

## Adoption Test

Before importing any adjacent pattern, answer:

1. Which exact Forge authority domain owns it?
2. Which principal may propose, approve, execute, attest, and release it?
3. Which canonical record and revision represent it?
4. How is Project/account scope derived server-side?
5. What data is intentionally excluded from model context?
6. What must remain compatible or migrate in place?
7. Which independent validation and failure-recovery cases prove it?

If these answers are absent, keep the idea in research rather than copying code or schema.
