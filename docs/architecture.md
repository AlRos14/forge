# Architecture

This document describes the current workspace implementation on the 0.9.1 source baseline, including migration V181 for Task and WorkUnit repository scopes. The ordered migration contract and invariants are in [Architecture V2](migration/architecture-v2.md); domain details are in [concepts](concepts/).

> The Orchestrator owns work boundaries. The harness owns cognition.

Forge coordinates durable programming work performed by human and Agent Actors. It does not own an Agent's model loop, native planning, tools, or private session protocol. A harness adapter translates between Forge Executions and that harness.

## Current domain

```text
Project
├── Repo*
└── Task*
    ├── TaskLifecycle
    ├── TaskRole*
    │   └── RoleMembership*
    ├── WorkUnit*
    ├── Execution*
    │   ├── Actor
    │   ├── Role
    │   ├── WorkUnit?
    │   ├── HarnessSession?
    │   └── Workspace?
    ├── ReviewReport / Artifact*
    ├── ValidationRun / Evidence*
    ├── Gate / GateEvaluation*
    └── Message* / Handoff* / Proposal* / Decision*
```

An Actor is a Human or Agent. An Agent has a harness identity and selectable profile. A TaskRole defines responsibility for one Task; its versioned configuration can grant the closed capabilities `formal_review`, `orchestration`, and `request_rework`. RoleMembership records which Actors may participate. Neither membership, role name, nor a legacy Task field assigns executable scope. A WorkUnit names the Role and, when allocated, the Actor for that scope.

Projects can store default TaskRoles, coordination modes, and workers as a
template. Ordinary Tasks and subtasks copy the template atomically into their
own TaskRole and RoleMembership records. Existing Tasks remain independent when
the Project template changes. Human defaults require current Project
membership; Agent defaults require ownership by the canonical Project owner,
so global discovery does not create cross-account execution authority.

An Execution records one Actor, Role, and Task, with an explicit WorkUnit, Workspace, and HarnessSession when applicable. Execution purpose is retired; historical purpose values remain migration evidence only. Parent Execution records lineage; it does not grant authority or imply session reuse. Execution identity and configuration snapshots are historical.

Tasks have no classification or Task-level repository access mode. An Agent
that passes the current TaskRole, ownership, and WorkspaceLease checks receives
the `repository_write` capability for its repository WorkUnit. Task instructions
describe the intended outcome; they do not select repository capability.

A Task has an ordered repository scope with one primary repository when the
scope is non-empty; affected and context repositories remain in that Task's
Project. A WorkUnit may explicitly select a subset of the Task's writable
repositories. An empty WorkUnit selection means all writable Task repositories;
context repositories are never writable.

## Authority and lifecycle

`TaskLifecycleService` owns the aggregate Task state and its versioned transition receipts. The current states are `backlog`, `ready`, `active`, `blocked`, `ready_to_merge`, `merging`, `done`, and `cancelled`. The `task.status` column was removed in V129; it is not an active projection or a second state authority.

TaskRole and RoleMembership govern eligibility. WorkUnit allocation names the executable Actor and Role. A current active RoleMembership is required for an assigned Agent to run. Membership alone does not grant access to another WorkUnit's Workspace, lease, terminal, or repository scope.

Forge's event-driven Orchestrator reacts to durable domain events through bounded outputs. Dedicated Task coordination turns have separate typed contracts for plans, review and validation findings, Human decisions, and external PR feedback. The coordinator does not grant its own authority, change a review verdict, or merge a repository. The dispatcher may apply only a preauthorized fallback Agent after the exact route and retry checks; this is a separate domain operation, not an Orchestrator side effect.

Task coordination adds a bounded preparation turn over the same durable wake and
Execution machinery. A Human authorizes a mode, exact coordinator TaskRole and
Actor, current team digest, and envelope. The `task.coordination_requested`
event admits an exact-Actor workspace-free wake; its Execution returns one
`work_unit_plan` Artifact. The model cannot create TaskRoles, memberships,
capabilities, WorkUnits, or worker Starts. Deterministic materialization validates
the exact plan, envelope, team, Task version, and authorization version and
creates WorkUnits, dependencies, review subjects, and Artifact input pins in one
transaction with an idempotent receipt. V160 marks the authorization stale on
any TaskRole, capability, or membership mutation; materialization and Provider
Start revalidate revocation, team state, dispatch, lifecycle, and the exact
coordination request under their own write locks. `manual` remains the default
for existing Tasks, and no coordination turn is triggered retrospectively.
V168 distinguishes immutable request kinds: existing requests remain
`work_unit_plan`; `review_findings` pins one ReviewReport/digest and its reviewer
producer Execution. The coordinator consumes only that report and applies one
bounded `correct|recheck|human` decision; the immutable triage and terminal
`triaged` request state commit together without a plan Artifact. Batch requests
retain a typed `review_triage` origin and the same authority fences. `recheck`
uses a fresh formal review of the unchanged frozen subject, not an arbitrary
verification command. A later request-changes report opens another exact turn;
a passing report remains subject to the current review line and Gate policy.
Neither the triage nor technical completion confers acceptance or Task Done.
Quality requests and the versioned intervention circuit are separate durable
records; they do not replace TaskLifecycle, review, Gate, or collaboration
authority. A durable quality consumer follows the exact report/rework chain:
delivery corrections only link the WorkUnit created by that exact ReviewReport,
repeated rounds stay on the same chain, and resolution requires the independent
review verdict to pass. Specialist requests create linked WorkUnits; plan
revisions open a coordination turn. Quality-linked WorkUnits complete
technically when their Execution finishes, but a delivery correction is never
resolved by that completion alone.
Coordinator verification uses exact pins in the current authorization envelope. The ValidationCheckRunner executes catalog checks against an accepted WorkUnit result or Task integration subject; optional baseline runs produce deterministic comparisons. Failed or inconclusive checks can wake a source-bound coordinator request. A structured Human decision records exact evidence, option, version, and resume binding. Forge never runs a command supplied by the coordinator.

The GitHub PR feedback loop uses a Project owner/admin-configured policy and an owned SCM credential handle. Policy and credential references are frozen for each remote PR admission. External feedback is stored as untrusted provider data; only trusted, current-head items enter coordinator triage, while public SSE events omit provider-authored bodies. Same-PR rework returns through normal WorkUnit, review, Gate, and integration checks. Push or summary actions default to Human approval; automatic execution requires an owner/admin repository policy. Forge does not provide a merge action.

Coordination envelope v2 also accepts explicit phase-3 preauthorizations for
bounded micro-fixes, formal reviewers, per-TaskRole fallback Agents, and
coordinator replanning. Unset fields remain absent from canonical serialization,
so historical v1 digests stay stable. Pin validation requires fallback/reviewer
Agents to be active members of the exact TaskRole under the Project owner's
account and excludes the coordinator Agent from fallback chains. V180 persists
immutable fallback and micro-fix receipts; a micro-fix bounds receipt can move
once from `pending` to a terminal outcome. The coordinator-replan ledger remains
the V172 record. Runtime handling admits cross-Agent fallback only after an
exhausted route and revalidates the exact TaskRole, membership, envelope, and
Provider Start authority. A bounded micro-fix records its source and exact
correction WorkUnit, measures the completed result diff against the reviewed
commit, and sends every result through formal review and a fresh Gate. A
coordinator replan opens a typed plan request under the `CoordinatorReplan`
origin; Rust compares objective, team, WorkUnit budget, repository/path scope,
and envelope grants. A scope or authority delta opens an exact Human decision
and remains unmaterialized until approval. The replan ledger enforces two
proposals per Task.

Each quality chain also has a persisted quality-routing budget (four for
historical Tasks, sixteen by default for new Tasks).
Review requests on one `work_unit_review_line` share the same route identity
across new reports, corrections, category changes, and destination-role
changes. When three unresolved requests target roles A, B, A in order, Forge
records a durable `requires_human` escalation with the exact request and
TaskRole IDs. Budget exhaustion, a pending escalation, and failures discovered
by the durable quality consumer also persist a concrete reason on the request.
Replays do not consume another
global or route round; `BEGIN IMMEDIATE` serializes concurrent admissions.
The rework/reassessment round limit remains separate (eight for historical
Tasks, sixteen by default for new Tasks). Migration V161
backfills route accounting for existing quality requests without changing their
states or replaying them.

Materialization prepares runnable, assigned Agent repository scopes through the
existing WorkUnit workspace service before requesting launch. The running
dispatcher prepares later dependent plan scopes and exact quality/rework
corrections when they become ready. Manual Tasks and unassigned or Human scopes
are not automatically prepared. A materialization replay reuses its receipt and
can retry preparation and launch; it never duplicates WorkUnits. Workspace
preparation does not itself grant an Execution or provider Start.

Materialization also creates a durable review line and a WorkUnit-scoped
`review_approval` Gate for each formal review subject. Any materialized WorkUnit
that directly depends on the review consumes that Gate and line, so it cannot
start until the latest review verdict passes. Technical completion by the
Orchestrator runtime is opt-in and fenced: it requires a current non-manual
Task coordination authorization, no revocation or team staleness, and either a
plan-materialized delivery, an exact quality-request-linked correction, or a
completed automatic-rework intent. Historical/manual Tasks and arbitrary
Artifact/review rows stay untouched. The exact authorization version, mode,
revocation, and team staleness are revalidated again inside the same
`BEGIN IMMEDIATE` transaction that completes the WorkUnit and freezes its
result, so a revocation or mode change winning the race leaves zero automatic
effect. Completion is not approval.

The current product includes Task coordination, quality routing, coordinator verification, PR feedback and same-PR rework, and the phase-3 fallback, micro-fix, and replan paths. Phase-specific implementation and verification evidence is recorded in the [coordinator verification and PR loop plan](plans/coordinator-verification-and-pr-loop.md) and [delivery fluency plan](plans/autonomous-delivery-fluency.md). The phase-3 plan records its new tests and builds as NOT RUN; implementation presence does not establish full-branch CI or real-provider acceptance.

Automatic Orchestrator execution requires the Agent owner and Task Project owner to be the same account, with a current Project owner/admin/member membership. Global Agent visibility supports discovery and TaskRole membership; it does not grant cross-account execution. Forge has no cross-account Agent execution grants, so a missing or mismatched owner or membership fails closed at Execution admission and the transactional Provider Start claim.

## Harnesses and Executions

The [Project Copilot](concepts/project-copilot.md) is an explicitly configured
harness-bound Agent helping Humans organize a Project. Its persistent
conversation and Project management runs are separate from Task Executions;
they create no synthetic Task, TaskRole, WorkUnit or WorkspaceLease. A live,
Project-scoped MCP delegation permits bounded WorkUnit management with exact
Agent attribution and current ownership checks. Task execution and lifecycle
authority continue to use the contracts below.

`HarnessAdapter` is the boundary for supported harness detection, profile/configuration translation, capability evidence, Start/Resume, cancellation, normalized results, and usage observations. Forge fails closed when a required capability is unknown or unsupported. It does not read a harness's private credential store.

A Human Execution has no HarnessSession. An Agent Execution references a HarnessSession explicitly when continuity is used. Resume never selects a session by Task, Role, Agent model label, or latest Execution.

Execution admission and external provider Start are separate durable boundaries. Forge commits the Running Execution, its exact WorkspaceLease when required, the `execution.started` event, and a pending Provider Start receipt together. Before sending Start, it atomically records that one attempt has been claimed. An accepted or rejected result is replayed from the receipt. An uncertain result is exposed as `execution_start_uncertain` and does not authorize another provider Start. Execution updates, exact review-subject freezing, HarnessSession result binding, durable event checkpoints, and Orchestrator attempt reservation acquire the SQLite writer before reading the state they will mutate. This prevents WAL read-to-write upgrade failures while preserving the same authority and version checks.

## Workspaces and integration

A repository WorkUnit that requires integration receives its exact isolated Workspace and short-lived WorkspaceLease. Its branch and base commit are tied to that WorkUnit. Completing it does not integrate changes. Integration is a separate serialized Git operation with a durable outcome.

A non-repository WorkUnit may have no Workspace and no WorkspaceLease. The current Agent path requires an online embedded host and a supported local CLI adapter. Unsupported remote daemon/provider combinations are rejected before the Execution enters `running`. This path is not a general remote no-Workspace execution feature.

Dependencies form a same-Task directed acyclic graph. Readiness is derived from WorkUnit state, dependencies, Execution results, workspace state, and integration outcomes. A Task transition or completed Execution does not silently complete or integrate a WorkUnit. An open WorkUnit with a completed result is `awaiting_acceptance` and receives no automatic retry until that result is accepted or the unit is cancelled.

Task dispatch is a durable intent owned by TaskLifecycle authority. An authenticated launch binds the exact Task and TaskLifecycle versions and an idempotency key. The TaskDispatcher admits exact assigned Agent WorkUnits while the intent is `running`; pause blocks new admissions and provider starts, and resume dispatches currently ready units. A Human-assigned WorkUnit is pending human work with no synthetic Execution. A bounded rework operation over an exact request-changes ReviewReport creates correction plus new review units and consumes one per-Task round atomically. Automatic routes are explicit, durable, replay-safe, and revalidated after pause or configuration changes.

The optional [Project plan](concepts/project-plan.md) organizes ordinary Tasks
as versioned work packets with canonical dependency edges, proposals, and
provenance. It does not replace the WorkUnit DAG inside a Task or TaskLifecycle.
Applying a proposal can create or map Tasks but does not authorize work. A
separate owner/admin batch authorization grants bounded scheduling; readiness,
dispatch, and batch state remain distinct projections.

TaskCoordination authorizations minted by a Project batch retain an immutable
binding to that exact batch. The generic Task coordination request route rejects
such a version with `409 project_plan_batch_inactive`, including after the batch
is paused, revoked, or completed; a fresh explicit TaskCoordination authorization
restores the standalone request path. Scheduler, Quality Replan, Human Replan, and ReviewTriage
continuations retain their exact claim token or source Artifact,
producer Execution, or explicitly selected plan Artifact. Human Replan requires
approval valid in the active authorization/batch and an exact materialization
receipt when the plan is materialized. Multiple plans may remain valid in one
authorization; authority is never selected by latest timestamp. The origin is
recorded directly and is never inferred from an authorization-version change.

While that authorization version remains batch-bound, generic Task events do
not create Agent coordinator wakes. Batch-bound Agent coordination starts only
from its typed Scheduler, Quality Replan, Human Replan, or ReviewTriage request; Human-targeted
collaboration remains available. Terminal events for existing batch-linked
coordinator Executions continue to reconcile against their exact durable wake,
attempt, and request. A fresh explicit Human authorization with a new version
restores generic standalone coordination after the batch is inactive. Provider
Start checks the batch request and Execution link in its write transaction, so
a generic wake racing with batch authorization fails closed.

## Review, validation, and Gates

Formal review is an Execution admitted under a TaskRole snapshot with `formal_review`. Its ReviewReport is an Artifact with exact Execution provenance. Review judgment remains distinct from deterministic checks. A durable review line follows the source result and each correction; a typed approval requirement is satisfied only by a favorable report for the line's latest result.

A ValidationRun represents a deterministic command or check and its exact workspace, commit, status, output, and Evidence. Project owners/admins publish immutable validation-check catalog versions; an authorized coordinator can request only an exact check pinned in the Task envelope, and Forge can run configured checks after integration. Optional baseline comparisons distinguish regressions from existing failures. Agent validation-related cognition remains ordinary WorkUnit work, not deterministic validation evidence.

A Gate is a deterministic policy over exact durable records. Its immutable GateEvaluation records the exact policy revision, inputs, and outcome. Policy schema 2 can require passing ValidationRuns for pinned check keys and definition digests; each check resolves against the exact candidate subject. A Gate does not run a test, perform a review, integrate a WorkUnit, or merge a Repo. Lifecycle transitions and merge admission use the exact current evaluation where required. See [review and validation](concepts/review-and-validation.md) and [Gates](concepts/gates.md).

## Persistence and history

SQLite is the current persistence layer. Numbered migrations V001–V181 and their receipts remain in migration order; do not edit historical migration files (V176 is unused because V177–V179 already exist). V181 adds ordered Task repository scopes, mirrors the primary repository to `task.repo_id`, and stores explicit WorkUnit repository selections. PR13 preserved exact legacy provenance in dedicated history tables and removed retired runtime schemas after history cutovers. V135 repairs the canonical Project owner membership representation without inventing owners for ownerless Projects. V149–V156 add versioned TaskRole capability configuration and admission snapshots, durable automatic rework intents, Project defaults, review lines, exact approval inputs, immutable Artifact input/manifests, and audited reconciliation with an exact manual rework receipt. V157 advances a review line from its exact reviewed result across reports. V158–V160 add Task coordination, plans, materialization, quality requests, interventions, and resolution records; V161 adds durable quality-route counters and Human-escalation evidence. V167 adds optional Project plans and batch authorization. V171–V172 add the validation catalog, request kinds, and coordinator-replan ledger; V173–V175 add SCM credential references, PR feedback, Human decisions, and same-PR rework records. V180 adds immutable fallback and micro-fix receipts.

Historical `agent_session_history`, immutable Execution records, ProjectRelease snapshots, Artifact and Evidence references, HarnessSession identities, cross-record IDs, migration decisions, ADRs, and upstream license attribution remain readable history. History does not regain runtime authority. ProjectRelease is a read-only historical snapshot and does not create a current Charter, milestone, readiness workflow, or release operation.

V167 stores immutable source bindings for TaskCoordination grants and batch
requests. A grant records the exact Project, batch, Task, authorized Human,
versions, Task objective, team/envelope snapshot, and coordinator Role/Actor in
the same write as the authorization change. Each batch request additionally
records whether it came from a scheduler claim, a Quality Replan Artifact and
producer Execution, an approved Human Replan Artifact, or an exact ReviewTriage
Report and reviewer producer Execution.

Configuration or data path changes can affect installed users. The current local identifiers include the `forge` brand, `FORGE_*` variables, `~/.forge`, `forge.yaml`, `forge.db`, `jwt_secret.bin`, `server.json`, workspace roots, and log locations. Changing them requires a separately reviewed discovery and preservation strategy; no such rename has been authorized.

## Runtime topology

| Part                                        | Current responsibility                                                                                                           |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `forge-cli` (`forge`)                       | Local server, web assets, REST and optional MCP endpoint, and embedded daemon startup.                                           |
| `api` and `api-types`                       | REST routes, permissions, serialization contracts, and public request/response types.                                            |
| `services` and `db`                         | Domain authority and SQLite persistence. `TaskLifecycleService` owns Task lifecycle decisions.                                   |
| `events`                                    | Event delivery and wake hints; durable domain records remain authoritative.                                                      |
| `mcp-server`                                | MCP tools over the same domain services and permissions.                                                                         |
| `forge-client` (`forge-ctl`)                | Command-line client for the REST API. It currently has no WorkUnit creation command.                                             |
| `forge-daemon`, `cli-adapters`, `executors` | External host transport, process supervision, and harness translation.                                                           |
| `workspace`, `git`                          | Repository isolation, path guards, workspace leases, and serialized Git integration.                                             |
| `config`                                    | Local settings, data paths, credentials, and defaults.                                                                           |
| `web/`                                      | React interface for Projects, Tasks, Executions, Agents, daemons, operations, settings, and historical ProjectRelease snapshots. |

## Public and local surfaces

The server binds by default to `127.0.0.1:0`: it selects an available loopback port and persists the bound address in `server.json` under the data directory. A later launch reuses that address when available. `FORGE_SERVER_BIND` or the config file can provide a fixed address.

The local configuration file is `~/.forge/forge.yaml` by default. Precedence is defaults, config file, environment variables, then explicit CLI overrides. `FORGE_DATA_DIR` selects the local data directory; `forge --data-dir` overrides it. The workspace root defaults to the operating system temporary directory under `forge/worktrees`. SQLite is stored as `forge.db`; the generated JWT secret is `jwt_secret.bin`; server discovery state is `server.json`. Execution logs, sessions, and media are also kept under the data directory. CLI login credentials use `<data-dir>/forge_ctl_credentials.json`. MCP client configuration is written to the selected client's project, local, or user config file.

The public surfaces are:

- REST under `/api/v1`, with authenticated resource authorization.
- MCP through the server's registered tool set; `tools/list` is authoritative.
- `forge-ctl`, which calls the REST API. It supports Projects, Repos, Tasks, Roles, Agents, provider settings, daemons, and MCP client setup; WorkUnits are managed through REST/MCP.
- The web UI, whose actual routes and navigation are defined in `web/src/router.tsx` and `web/src/components/app-shell.tsx`.
- Authenticated SSE at `/api/v1/events`, a bounded projection of durable events plus an allowlist of safe runtime hints.

The API and UI do not expose the retired Main Agent, Project Agent, Agent Chat, Product Genesis, Project OS, Attention, or legacy WorkflowEngine as operational features. Historical references in migration records, old changelog entries, generated ProjectRelease snapshots, or upstream material are retained as history.

## Configuration defaults

| Setting                        | Default                                                                                                     |
| ------------------------------ | ----------------------------------------------------------------------------------------------------------- |
| Server bind                    | `127.0.0.1:0`; a free loopback port is selected and saved.                                                  |
| Data directory                 | `~/.forge` (or `.forge` when no home directory is available).                                               |
| Config file                    | `<data-dir>/forge.yaml`.                                                                                    |
| Database                       | `<data-dir>/forge.db`.                                                                                      |
| JWT secret file                | `<data-dir>/jwt_secret.bin`, created with restricted permissions when no configured secret is present.      |
| Server state                   | `<data-dir>/server.json`.                                                                                   |
| Workspace root                 | system temporary directory + `/forge/worktrees`.                                                            |
| Web assets                     | `web/dist` in a checkout, then installed `share/forge/web/dist`; `FORGE_WEB_DIST_DIR` overrides the lookup. |
| Default agent concurrency      | `1`.                                                                                                        |
| Default missed heartbeat limit | `3` intervals.                                                                                              |
| Default heartbeat interval     | `30` seconds.                                                                                               |

Supported settings are defined in `crates/config/`. Common environment variables include `FORGE_SERVER_BIND`, `FORGE_PUBLIC_BASE_URL`, `FORGE_DATA_DIR`, `FORGE_WORKSPACE_ROOT`, `FORGE_WORKSPACE_CLEANUP_DELAY_SECONDS`, `FORGE_AGENT_MAX_CONCURRENT_TASKS`, `FORGE_AGENT_HEARTBEAT_INTERVAL_SECONDS`, `FORGE_AGENT_MAX_MISSED_HEARTBEATS`, `FORGE_JWT_SECRET`, `FORGE_BCRYPT_COST`, `FORGE_CORS_ORIGINS`, and `FORGE_MEDIA_UPLOAD_LIMIT_BYTES`. `RUST_LOG` and `FORGE_LOG_FORMAT` configure logging.

## Container execution privilege

The container image runs Forge and its embedded shell as the unprivileged
`forge` user (UID/GID `10001`). The entrypoint starts as root only to inspect
`/data`, fails closed on an unapproved root-owned volume, and drops privileges
before `exec`; the application process is never root. An explicit
`FORGE_MIGRATE_ROOT_OWNED_DATA=1` one-time run authorizes ownership repair only
under `/data`. Workspace and repository mounts are never recursively rewritten.

Compose binds the host port to `127.0.0.1` by default. Public exposure requires
an explicit bind override, a reviewed proxy/TLS boundary, and the deployment
controls in [deployment](deployment.md). Do not infer daemon or Project
ownership for historical records during a container migration.

## Upstream

Forge originated from [ForgeAILab/forge](upstream.md). The origin and MIT attribution remain. This repository is independently maintained and does not promise architectural or release compatibility with upstream. See [upstream relationship](upstream.md).

## Task operating model and delivery

The [task operating model](concepts/task-operating-model.md) describes the current relationship between Project setup, explicit Task authorization, coordinator turns, assigned WorkUnits, review, validation, Human decisions, and delivery. Project Copilot remains a separate Project-scoped assistant and does not gain worker Execution or Task coordination authority. The [delivery fluency concept](concepts/delivery-fluency.md) summarizes the autonomous delivery path and its boundaries.

### Guided delivery and verification

Agent review independence is enforced through Task/role HarnessSession isolation; Human contributors cannot review their own result. Coordinators triage exact immutable findings through bounded typed outputs without becoming the approval authority. Validation checks, Human decisions, PR feedback, fallback, micro-fixes, and replans retain their own durable receipts and budgets. See [orchestration](concepts/orchestration.md), [review and validation](concepts/review-and-validation.md), [PR feedback](concepts/pr-feedback-loop.md), and [WorkUnits](concepts/work-units.md).

Phase 1 and phase 2 full CI results and the phase 3 NOT RUN boundary are recorded in the linked implementation plans. Acceptance with a real GitHub provider is not inferred from implementation or fixture evidence. The authority invariants remain in [Architecture V2](migration/architecture-v2.md).
