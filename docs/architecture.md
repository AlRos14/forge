# Architecture

This document describes the implementation after PR13. The ordered migration contract and invariants are in [Architecture V2](migration/architecture-v2.md); domain details are in [concepts](concepts/).

> The Orchestrator owns work boundaries. The harness owns cognition.

Forge coordinates durable programming work performed by human and Agent Actors. It does not own an Agent's model loop, native planning, tools, or private session protocol. A harness adapter translates between Forge Executions and that harness.

## Current domain

~~~text
Project
├── Repo*
└── Task*
    ├── TaskLifecycle
    ├── TaskRole*
    │   └── RoleMembership*
    ├── WorkUnit*
    ├── Execution*
    │   ├── Actor
    │   ├── Role and Purpose
    │   ├── HarnessSession?
    │   └── Workspace?
    ├── ReviewReport / Artifact*
    ├── ValidationRun / Evidence*
    ├── Gate / GateEvaluation*
    └── Message* / Handoff* / Proposal* / Decision*
~~~

An Actor is a Human or Agent. An Agent has a harness identity and selectable profile. A TaskRole defines responsibility for one Task; RoleMembership records which Actors may participate. Neither membership nor a legacy Task field assigns executable scope. A WorkUnit names the Role and, when allocated, the Actor for that scope.

An Execution records one Actor, Role, Purpose, and Task, with an explicit WorkUnit, Workspace, and HarnessSession when applicable. Parent Execution records lineage; it does not grant authority or imply session reuse. Execution identity and configuration snapshots are historical.

## Authority and lifecycle

`TaskLifecycleService` owns the aggregate Task state and its versioned transition receipts. The current states are `backlog`, `ready`, `active`, `blocked`, `ready_to_merge`, `merging`, `done`, and `cancelled`. The `task.status` column was removed in V129; it is not an active projection or a second state authority.

TaskRole and RoleMembership govern eligibility. WorkUnit allocation names the executable Actor and Role. A current active RoleMembership is required for an assigned Agent to run. Membership alone does not grant access to another WorkUnit's Workspace, lease, terminal, or repository scope.

Forge's Orchestrator reacts to durable domain events. Its implemented output set is bounded to Messages, Handoffs, limited WorkUnit creation, and Proposals. It does not directly steer or stop a harness, reassign work, merge repositories, or execute a Proposal or Decision. Human and Agent actions remain subject to the same domain authority.

## Harnesses and Executions

`HarnessAdapter` is the boundary for supported harness detection, profile/configuration translation, capability evidence, Start/Resume, cancellation, normalized results, and usage observations. Forge fails closed when a required capability is unknown or unsupported. It does not read a harness's private credential store.

A Human Execution has no HarnessSession. An Agent Execution references a HarnessSession explicitly when continuity is used. Resume never selects a session by Task, Role, Agent model label, or latest Execution.

Execution admission and external provider Start are separate durable boundaries. Forge commits the Running Execution, its exact WorkspaceLease when required, the `execution.started` event, and a pending Provider Start receipt together. Before sending Start, it atomically records that one attempt has been claimed. An accepted or rejected result is replayed from the receipt. An uncertain result is exposed as `execution_start_uncertain` and does not authorize another provider Start.

## Workspaces and integration

A repository WorkUnit that requires integration receives its exact isolated Workspace and short-lived WorkspaceLease. Its branch and base commit are tied to that WorkUnit. Completing it does not integrate changes. Integration is a separate serialized Git operation with a durable outcome.

A non-repository WorkUnit may have no Workspace and no WorkspaceLease. The current Agent path requires an online embedded host and a supported local CLI adapter. Unsupported remote daemon/provider combinations are rejected before the Execution enters `running`. This path is not a general remote no-Workspace execution feature.

Dependencies form a same-Task directed acyclic graph. Readiness is derived from WorkUnit state, dependencies, Execution results, workspace state, and integration outcomes. A Task transition or completed Execution does not silently complete or integrate a WorkUnit.

## Review, validation, and Gates

Formal review is an Execution under Role `reviewer` and Purpose `review`. Its ReviewReport is an Artifact with exact Execution provenance. Review judgment remains distinct from deterministic checks.

A ValidationRun represents a deterministic command or check and its exact workspace, commit, status, output, and Evidence. In this implementation snapshot the public REST, MCP, CLI, and web surfaces expose ValidationRuns for reading; they do not expose a command to start one. An Agent Execution with Purpose `validate` is cognitive investigation, not deterministic validation evidence.

A Gate is a deterministic policy over exact durable records. Its immutable GateEvaluation records the exact policy revision, inputs, and outcome. A Gate does not run a test, perform a review, integrate a WorkUnit, or merge a Repo. Lifecycle transitions and merge admission use the exact current evaluation where required.

## Persistence and history

SQLite is the current persistence layer. Numbered migrations V001–V134 and their receipts remain in migration order; do not edit historical migration files. PR13 preserved exact legacy provenance in dedicated history tables and removed retired runtime schemas after history cutovers.

Historical `agent_session_history`, immutable Execution records, ProjectRelease snapshots, Artifact and Evidence references, HarnessSession identities, cross-record IDs, migration decisions, ADRs, and upstream license attribution remain readable history. History does not regain runtime authority. ProjectRelease is a read-only historical snapshot and does not create a current Charter, milestone, readiness workflow, or release operation.

Configuration or data path changes can affect installed users. The current local identifiers include the `forge` brand, `FORGE_*` variables, `~/.forge`, `forge.yaml`, `forge.db`, `jwt_secret.bin`, `server.json`, workspace roots, and log locations. Changing them requires a separately reviewed discovery and preservation strategy; no such rename has been authorized.

## Runtime topology

| Part | Current responsibility |
|---|---|
| `forge-cli` (`forge`) | Local server, web assets, REST and optional MCP endpoint, and embedded daemon startup. |
| `api` and `api-types` | REST routes, permissions, serialization contracts, and public request/response types. |
| `services` and `db` | Domain authority and SQLite persistence. `TaskLifecycleService` owns Task lifecycle decisions. |
| `events` | Event delivery and wake hints; durable domain records remain authoritative. |
| `mcp-server` | MCP tools over the same domain services and permissions. |
| `forge-client` (`forge-ctl`) | Command-line client for the REST API. It currently has no WorkUnit creation command. |
| `forge-daemon`, `cli-adapters`, `executors` | External host transport, process supervision, and harness translation. |
| `workspace`, `git` | Repository isolation, path guards, workspace leases, and serialized Git integration. |
| `config` | Local settings, data paths, credentials, and defaults. |
| `web/` | React interface for Projects, Tasks, Executions, Agents, daemons, operations, settings, and historical ProjectRelease snapshots. |

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

| Setting | Default |
|---|---|
| Server bind | `127.0.0.1:0`; a free loopback port is selected and saved. |
| Data directory | `~/.forge` (or `.forge` when no home directory is available). |
| Config file | `<data-dir>/forge.yaml`. |
| Database | `<data-dir>/forge.db`. |
| JWT secret file | `<data-dir>/jwt_secret.bin`, created with restricted permissions when no configured secret is present. |
| Server state | `<data-dir>/server.json`. |
| Workspace root | system temporary directory + `/forge/worktrees`. |
| Web assets | `web/dist` in a checkout, then installed `share/forge/web/dist`; `FORGE_WEB_DIST_DIR` overrides the lookup. |
| Default agent concurrency | `1`. |
| Default missed heartbeat limit | `3` intervals. |
| Default heartbeat interval | `30` seconds. |

Supported settings are defined in `crates/config/`. Common environment variables include `FORGE_SERVER_BIND`, `FORGE_PUBLIC_BASE_URL`, `FORGE_DATA_DIR`, `FORGE_WORKSPACE_ROOT`, `FORGE_WORKSPACE_CLEANUP_DELAY_SECONDS`, `FORGE_AGENT_MAX_CONCURRENT_TASKS`, `FORGE_AGENT_HEARTBEAT_INTERVAL_SECONDS`, `FORGE_AGENT_MAX_MISSED_HEARTBEATS`, `FORGE_JWT_SECRET`, `FORGE_BCRYPT_COST`, `FORGE_CORS_ORIGINS`, and `FORGE_MEDIA_UPLOAD_LIMIT_BYTES`. `RUST_LOG` and `FORGE_LOG_FORMAT` configure logging.

## Container execution privilege

The published Docker image currently runs Forge as root, and Compose binds the
server port on all host interfaces. The embedded shell therefore inherits the
container's root identity. This PR restricts which account can reach that shell
through Forge authority, but does not yet drop container privileges. A safe
non-root image requires a forward-compatible ownership strategy for existing
`/data` volumes and workspace mounts; treat container privilege reduction as a
separate operational migration. Do not infer daemon ownership for historical
records during that migration.

## Upstream

Forge originated from [ForgeAILab/forge](upstream.md). The origin and MIT attribution remain. This repository is independently maintained and does not promise architectural or release compatibility with upstream. See [upstream relationship](upstream.md).
