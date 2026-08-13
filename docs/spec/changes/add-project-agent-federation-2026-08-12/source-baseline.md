# Source baseline

## Product package

The input archive was `/Volumes/Data/Downloads/forge-v2-project-agent-federation-compact.zip`, dated 2026-08-11. It contains five design documents plus `MANIFEST.sha256`. The referenced Codex session verified its manifest before the LCM extraction began.

| File | SHA-256 |
|---|---|
| `ONE-PAGER.md` | `4ce62e8923479a48db3e538c3d66a0271e6eb0ed7aac45e386ed93c24e7a591c` |
| `SPEC.md` | `7ae97c5cf21f8f7e6cb50fdd32586585204c768d293d6d22d7b7c637b97bfaa2` |
| `MVP-PLAN.yaml` | `bb9cf16188a9d8570235068299aeea5b4280ab3ce8f660e7b834a95f70d425da` |
| `ACCEPTANCE.md` | `2481530fe74897c6425c0197f7b9c95c2f4fa77e0af7c13518164b7364080dcd` |
| `DECISIONS.md` | `fd33c4e6653778efec6100b3d7bd5c22a0785c4ea6f492b719c2ef23b73b32ff` |

The imported documents are design input, not authoritative repository specs. This change deliberately overrides their feature-flag/mixed-mode rollout decision to comply with Forge repository policy.

## User-directed product correction

Live acceptance was followed by an explicit product correction on 2026-08-13. The authoritative interaction model is one global Main Agent, exactly one Project Agent for each operational Project, a left-side switcher among those agents' durable chats, and an always-available bottom-right global-chat launcher. The Main Agent performs discovery, configured web research, global Project organization, and explicit handoff. The Project Agent owns setup and Task management within its Project. General Rooms, participants, addressing, and bounded multi-agent rounds are not part of the product.

This direction supersedes Room-oriented portions of the imported package while retaining its durable identity, scoped memory, runtime, commitment, event, and Task-delegation goals.

## Prior design and implementation session

Codex session: `019ff28b-1649-74f2-946c-4e0ede350789`.

That session established the ownership split and then implemented the reusable LCM extraction in the sibling Agent Runtime worktree. Its final reported gates included workspace tests/doctests, strict Clippy, Rust 1.86 builds, dependency checks, LCM conformance, replay/recovery coverage, and neutral consumer gates including Open Forge. At implementation start, the completed change was revalidated and committed as Agent Runtime revision `a7075b1d2dd1cee05db63bc480ff46b0f97ec239`. Forge MUST pin that exact revision (or a later explicitly reviewed replacement) and MUST NOT pin the pre-LCM baseline `1d6960e90dd29b48a83e863ca8768811e2f25a44`.

## Canonical LCM implementation

Active sibling change: `../agent-runtime/docs/spec/changes/add-lossless-context-memory-2026-08-11/`.

The reusable boundary consists of:

- `agent-runtime-lcm`: opaque timeline/entry/node identities, least-authority reader/writer store contracts, immutable timeline entries, transactional summary DAG mutation, bounded expansion, deterministic projection, and convergence-guaranteed summarization;
- `agent-runtime`: `LcmTimelineBinding`, `LcmCoordinator`, checkpointed idle/hard-pressure integration, `SessionHandle::expand_lcm`, lifecycle events, and run-manifest/replay integration;
- `agent-runtime-testkit`: generic store conformance for authorized/unauthorized views, append idempotency, atomic CAS, projection, expansion, and recovery behavior.

Forge adoption MUST pin Agent Runtime revision `a7075b1d2dd1cee05db63bc480ff46b0f97ec239` and run both the neutral `consumer_open_forge` gate and Forge's product adapter tests. The neutral gate and all 60 `agent-runtime-lcm` package tests passed against that revision at implementation start.

## Forge dependency and toolchain baseline

The workspace manifest pins `agent-runtime = 0.1.0` from
`https://github.com/ForgeAILab/agent-runtime.git` at exact revision
`a7075b1d2dd1cee05db63bc480ff46b0f97ec239`. The Forge-owned
`forge-agent-host` crate declares Rust 1.86 as its minimum supported toolchain.
Implementation validation began on `rustc 1.97.1 (8bab26f4f 2026-07-14)` and
`cargo 1.97.1 (c980f4866 2026-06-30)`.

The sibling checkout is clean at the pinned revision, and revision
`a7075b1d2dd1cee05db63bc480ff46b0f97ec239` is published on Agent Runtime's
`origin/main`. Local resolution uses only the gitignored `.cargo/config.toml`
path patch documented in `docs/getting-started.md`. Release-lock validation
temporarily removed that patch, resolved every Agent Runtime crate from the
exact Git revision with `cargo tree --locked`, and completed
`cargo check --locked -p forge-agent-host` against the remote source. The local
patch was restored after the release-source gate.

## Nyx provenance

Agent Runtime's LCM donor revision is Nyx `9614842d8f614d7d41e00d8e73ed3d042764d451`. Forge may reuse documented retrieval ideas—namespaced kinds, bounded scope/category retrieval, backend-neutral embedding interfaces—but MUST NOT copy Nyx's permissive “unscoped matches every scope” behavior or in-place memory upsert semantics into federation memory.

## Direct Forge host decision

`../tui` was inspected as a possible Agent Runtime host-composition reference. This change does not adopt its host packages, presentation code, dependency graph, or release cadence. Forge composes the LCM-capable Agent Runtime directly in a Forge-owned embedded host, so no TUI or Smith revision is a prerequisite. Local sibling testing uses only git-ignored Cargo patch configuration; committed Forge manifests use an immutable Agent Runtime Git revision or released version.

## Forge live acceptance baseline

The pre-correction implementation was exercised end to end with Smith as a real Task executor and with `ap-browser` against the local Forge UI. The complete report is `test/live-agent-20260813-0057/ACCEPTANCE.md`; proof images are under `test/live-agent-20260813-0057/browser-proof/` and representative images were attached to Task `4c01be61-671a-4bf3-8aab-a040772ae724`.

The live Task path succeeded: Project/repository creation, Task creation and assignment, execution, validation, review, merge, history, and proof attachment all completed. Responsive UI checks covered 1280, 768, and 375 pixel widths, dark mode, keyboard navigation, Agent detail, context inspection, Mission Control, Task overview, review evidence, and raw run events.

Two release-blocking defects were reproduced and are design inputs for this revision:

- an account-Room turn failure could not commit its failure state, remained silently leased, and retried across restart without a visible assistant/error outcome;
- a Project Worker membership could be marked primary because uniqueness applied only to primary Stewards, contradicting the intended single Project manager invariant.

The singular binding and explicit finite turn-job requirements directly close those ambiguity classes. The baseline is evidence of what exists now, not evidence that the Room surface is approved to ship.
