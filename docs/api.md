# API Reference

Forge exposes the versioned REST API at `/api/v1`. The public contract follows
the current Project, Task, Actor, TaskRole, RoleMembership, Execution,
HarnessSession, WorkUnit, Workspace, collaboration, Artifact, Review,
Validation, Evidence, Gate, and TaskLifecycle records.

> The orchestrator owns work. The harness owns cognition.

Plan PR12 is a breaking public-surface change. Main/Project Agent, Product
Genesis, Agent Chat, Attention, Charter, baseline, milestone governance,
legacy review rows, workflow state, and native runtime controls are not
operational REST resources. Their historical storage remains preserved for
PR13. Unknown `/api/v1` paths return the normal API `404 not_found` response;
removed routes are not retained as aliases or `410` dispatchers. Request DTOs
reject unknown legacy fields.

## Authentication, errors, and pagination

REST resources require a Bearer access token unless the route is part of the
public authentication or OAuth bootstrap flow. The browser EventSource passes
its access token as `?token=` because the native EventSource API cannot set an
Authorization header.

Errors use `{ "code": string, "message": string, "details": object | null,
"request_id": string }`. Common outcomes are `400` for invalid input or an
unsupported lifecycle request, `401` for missing/invalid credentials, `404`
for absent or invisible records, `409` for version or authority conflicts,
and `422` for malformed DTOs or unknown request fields.

Paginated endpoints return `items`, `has_more`, `next_cursor`, and, where
requested, `total_count`. Cursors are opaque. The default page size is 20 and
the maximum is 100.

## Projects, repositories, hooks, and media

| Method and path | Contract |
| --- | --- |
| `POST /projects` | Create an ordinary Project from `{ name }`. It does not create a Charter, Genesis session, baseline, Agent binding, or Task. |
| `GET /projects` | List Projects visible to the authenticated user. |
| `GET /projects/{id}` | Read a Project and its generic Project hook configuration. |
| `PATCH /projects/{id}` | Update Project identity, pause state, and supported generic hooks. |
| `DELETE /projects/{id}` | Delete a Project under the existing workspace cleanup contract. |
| `POST /projects/{id}/pause`, `POST /projects/{id}/resume` | Pause or resume the Project. |
| `GET /projects/{id}/repos`, `POST /projects/{id}/repos` | List or add repositories. |
| `GET /repos/{id}`, `PATCH /repos/{id}`, `DELETE /repos/{id}`, `POST /repos/{id}/sync` | Read, update, remove, or sync a repository. `PATCH` can set `pr_provider` and its polling/base URL configuration; setting the provider to `null` removes the provider configuration. Blank token input leaves the saved credential unchanged. Responses expose provider type, token presence, and polling interval without returning the secret. |
| `GET /projects/{id}/project_hook_runs` | Read generic Project hook run history. |
| `GET /projects/{id}/media`, `POST /projects/{id}/media` | List or upload Project media. |
| `GET /projects/{id}/media/{asset_id}` | Read an exact media asset. |
| `POST /projects/{id}/media/{asset_id}/redact`, `.../purge` | Record the existing media redaction or purge action. |
| `GET /projects/{id}/integration`, `POST /projects/{id}/integration`, `PATCH /projects/{id}/integration`, `POST /projects/{id}/integration/sync` | Manage the Project's external issue integration. Create accepts and GET/PATCH return `credential_env_var`, the environment variable name Forge reads during sync; the credential value is never returned or stored. Optional `default_implementer` is an exact `ActorRef`; imported issues create ordinary Tasks and, when configured, an `implementer` TaskRole with that Actor as a RoleMembership. Sync fails closed if the configured Actor is not valid for the Project. Lifecycle begins at the normal Task creation state; legacy `default_task_state` and singular assignment fields are rejected. |
| `GET /projects/{id}/members`, `POST /projects/{id}/members`, `PATCH /projects/{id}/members/{user_id}`, `DELETE /projects/{id}/members/{user_id}` | Manage Project membership. |

Project media remains a Project resource. The retired Task comment/media
attachment surface is not part of this API. Ordinary Task creation does not
inherit role assignments from historical Project settings; create exact
TaskRole and RoleMembership records through the Task role endpoints. Legacy
state-driven lifecycle hook settings are preserved as history but are no
longer dispatched. Generic Project hook rules are a separate target surface.

External issue integration secrets are accepted only on create/update and are
never returned. Historical default-state and default-assignee columns remain
unchanged for PR13, but do not control newly imported Tasks.

## Tasks and TaskLifecycle

| Method and path | Contract |
| --- | --- |
| `POST /projects/{project_id}/tasks` | Create an ordinary Task. Supported fields are `title`, `description`, `parent_task_id`, `task_type`, and `priority`. |
| `GET /projects/{project_id}/tasks` | List Tasks. Filter with `lifecycle_state`, `task_type`, `priority`, and `q`; order with the supported Task sort fields. `status`, `include_cancelled`, and `include_archived` are rejected. |
| `GET /tasks/{id}`, `PATCH /tasks/{id}`, `DELETE /tasks/{id}` | Read, update, or delete a Task. Updates require the Task `version`. |
| `POST /tasks/{id}/subtasks/reorder` | Reorder exact child Task IDs. |
| `GET /tasks/{id}/dependencies`, `POST /tasks/{id}/dependencies`, `DELETE /tasks/{id}/dependencies/{dep_id}`, `GET /tasks/{id}/dependents` | Read and update Task dependencies. |
| `GET /tasks/{id}/workspace`, `POST /tasks/{id}/workspace/reset`, `GET /tasks/{id}/diff` | Read or reset the Task workspace, or inspect its current diff. |
| `GET /tasks/{id}/lifecycle` | Read the authoritative aggregate TaskLifecycle and its version. |
| `POST /tasks/{id}/lifecycle` | Request a deterministic TaskLifecycle edge with `to_state`, `expected_lifecycle_version`, and `idempotency_key`; use `gate_evaluation_id` for an edge admitted by that exact evaluation. `reason_kind` and `reason_ref` must be supplied together, with non-empty values, or both omitted. |
| `GET /tasks/{id}/lifecycle/transitions` | Read exact durable transition receipts and facts. |
| `GET /tasks/{id}/gates`, `POST /tasks/{id}/gates` | List or create Task-scoped Gates. |
| `GET /gates/{id}`, `PUT /gates/{id}/policy`, `POST /gates/{id}/evaluate` | Read a Gate, revise its policy against the expected revision, or evaluate its exact current inputs. |
| `GET /gate-evaluations/{id}` | Read one immutable GateEvaluation and its frozen input facts. |
| `POST /tasks/{id}/merge` | Request merge admission using the exact satisfied GateEvaluation. The service rechecks its current policy and input identities under the Task integration lock. |
| `GET /tasks/{id}/task-roles`, `POST /tasks/{id}/task-roles`, `PATCH /tasks/{id}/task-roles/{role}` | Read or update TaskRole definitions. |
| `POST /tasks/{id}/task-roles/{role}/members`, `PATCH /tasks/{id}/task-roles/{role}/members/{membership_id}` | Add or update an exact RoleMembership. Memberships name a Human or Agent Actor. |

TaskLifecycle is aggregate progress. The database `task.status` column remains
only as a one-way storage projection pending PR13; REST lists, filters, task
responses, and transitions do not read it as authority. Workflow templates,
workflow states, transition logs, and legacy GateConfig do not advance
TaskLifecycle.

## Executions and HarnessSessions

| Method and path | Contract |
| --- | --- |
| `GET /tasks/{id}/executions` | List exact historical Task Executions. |
| `POST /tasks/{id}/executions` | Start an Agent Execution. The request must name `agent_id`, exact `role`, `purpose`, unrewritten `prompt`, and optional exact `input_artifact_ids`. The Agent must be an active member of that TaskRole. A Task with WorkUnits is rejected until an explicit WorkUnit-scoped start is available. |
| `GET /executions/{id}` | Read one exact Execution, including its Actor, role, purpose, HarnessSession reference, frozen execution configuration, and workspace identity. |
| `POST /executions/{id}/follow-up` | Create a child from the exact parent Execution. `agent_id` is required. A same-Agent continuation can reuse only the exact compatible HarnessSession; it never substitutes a current role member or latest Execution. |
| `POST /executions/{id}/cancel` | Cancel the exact Execution. |
| `GET /executions/{id}/logs`, `GET /executions/{id}/hook-logs` | Read the exact Execution's persisted log output and hook logs. |
| `GET /executions/{id}/usage`, `GET /tasks/{id}/usage` | Read Execution or aggregate Task usage. |
| `GET /workspaces/{id}/diff` | Read an exact Workspace diff. |

The public Execution projection identifies the participant through `actor_ref`;
it does not expose the legacy AgentSession identity. A Human Execution has no
HarnessSession. `purpose` describes why the Actor works; it grants no
permission. Unsupported harness capabilities fail closed.

## Review, ValidationRun, Evidence, and Artifact

Formal Review is exactly `Execution.role = reviewer AND
Execution.purpose = review`. The legacy Review table does not authorize a
decision or satisfy a Gate.

| Method and path | Contract |
| --- | --- |
| `POST /tasks/{id}/review-executions` | Start a Human Review Execution. The response names that exact Execution. |
| `GET /tasks/{id}/review-executions` | List exact reviewer+review Executions and their ReviewReport Artifact outputs. |
| `GET /review-executions/{id}` | Read one exact formal Review Execution and report. |
| `POST /review-executions/{id}` | Submit a ReviewReport to that exact reviewer+review Execution. |
| `GET /tasks/{id}/validation-runs`, `GET /validation-runs/{id}` | List or read exact ValidationRuns. |
| `GET /evidence/{id}` | Read exact Evidence and its producer ValidationRun identity. |
| `GET /tasks/{task_id}/artifacts`, `POST /tasks/{task_id}/artifacts` | List or create generic Artifacts with exact provenance. |
| `GET /artifacts/{id}` | Read one exact Artifact. |

ValidationRuns and Evidence are distinct from ReviewReports. A Gate consumes
exact input facts and immutable evaluations; it never uses a latest Review or
ValidationRun lookup.

## WorkUnits and generic collaboration

| Method and path | Contract |
| --- | --- |
| `GET /tasks/{task_id}/work-units`, `POST /tasks/{task_id}/work-units` | List or create Task-scoped WorkUnits. |
| `GET /work-units/{id}`, `PATCH /work-units/{id}` | Read or update an exact WorkUnit. |
| `POST /work-units/{id}/allocation`, `POST /work-units/{id}/status` | Allocate a WorkUnit or request its target state transition. |
| `GET /work-units/{id}/dependencies`, `POST /work-units/{id}/dependencies/{prerequisite_id}`, `DELETE /work-units/{id}/dependencies/{prerequisite_id}` | Read or update exact WorkUnit dependencies. |
| `GET /work-units/{id}/readiness` | Read deterministic WorkUnit readiness. |
| `POST /work-units/{id}/integrations` | Integrate an explicitly scoped WorkUnit under the existing integration lock. |
| `GET /tasks/{task_id}/messages`, `POST /tasks/{task_id}/messages`, `GET /messages/{id}` | Create or read generic collaboration Messages. |
| `GET /tasks/{task_id}/handoffs`, `POST /tasks/{task_id}/handoffs`, `GET /handoffs/{id}`, `POST /handoffs/{id}/status` | Create, read, or update generic Handoffs. |
| `GET /tasks/{task_id}/proposals`, `POST /tasks/{task_id}/proposals`, `GET /proposals/{id}`, `POST /proposals/{id}/withdraw` | Create or read generic Proposals. |
| `GET /tasks/{task_id}/collaboration/decisions`, `POST /tasks/{task_id}/collaboration/decisions`, `GET /decisions/{id}` | Record or read a Decision with exact Proposal provenance. |

These are generic domain records. They do not recreate Agent Chat, Charter,
AgentAction, readiness, or Main/Project Agent workflows.

## Agents, providers, credentials, and daemon transport

| Method and path | Contract |
| --- | --- |
| `GET /agents`, `POST /agents`, `GET /agents/{id}`, `PATCH /agents/{id}`, `DELETE /agents/{id}` | Inspect or manage external harness Agent identities. |
| `POST /agents/{id}/pause`, `POST /agents/{id}/resume`, `GET /agents/{id}/availability`, `GET /agents/{id}/usage`, `GET /agents/{id}/discovered-options` | Manage or inspect current Agent/provider capability. There is no manual usage-refresh writer. |
| `GET /agents/{id}/profiles`, `POST /agents/{id}/profiles/{profile_id}/select` | Inspect/select supported external Harness profiles. Native/embedded runtime profiles are hidden and cannot be selected. |
| `GET /providers/catalog`, `GET /providers`, `POST /providers`, `PATCH /providers/{id}`, `DELETE /providers/{id}`, `POST /providers/{id}/test`, `GET /providers/{id}/usage` | Inspect or manage provider configuration and usage. |
| `POST /provider-authorizations`, `GET /provider-authorizations/{id}`, `POST /provider-authorizations/{id}/cancel` | Manage provider authorization. Secret values are not returned. |
| `GET /executor-types`, `GET /executor-types/{type_name}/discovered-options`, `GET /clis` | Inspect supported executor/provider capabilities. |
| `GET /daemons`, `POST /daemons/register`, `GET /daemons/{id}`, `GET /daemons/{id}/connect`, `POST /daemons/{id}/report` | Register and communicate with daemon harness hosts. |

Credential and provider infrastructure remains usable after PR10. It does not
restore Forge-owned cognition or native runtime dispatch.

Agent roster and availability responses expose `active_execution_count`. It
counts only `running` Execution rows whose immutable Actor is that exact Agent;
TaskLifecycle state and TaskRole membership do not imply running work.

## Authentication, settings, operations, and notifications

| Method and path | Contract |
| --- | --- |
| `POST /auth/register`, `POST /auth/login`, `POST /auth/refresh`, `POST /auth/logout`, `GET /auth/me`, `PATCH /auth/me` | Account authentication and profile. |
| `GET /auth/tokens`, `POST /auth/tokens`, `DELETE /auth/tokens/{id}` | Manage personal access tokens. |
| `GET /users/search` | Search users available for Project membership. |
| `GET /admin/users`, `PATCH /admin/users/{id}`, `DELETE /admin/users/{id}`, `GET /admin/settings`, `PUT /admin/settings/{key}`, `DELETE /admin/settings/{key}` | Administrative account/settings operations. |
| `GET /settings`, `PUT /settings` | Read or update supported server settings. |
| `GET /config/mcp`, `POST /config/mcp` | Read or update MCP connection configuration. |
| `GET /operations/status`, `POST /operations/refresh` | Read or refresh operational health. Blocked Tasks carry TaskLifecycle state/version and an exact transition ID when a current receipt exists. Retry pressure lists exact immutable retry receipts and source/receipt DomainEvent IDs; it does not count legacy transition-log rows or read Task status. Active Executions expose only their exact `harness_session_id`. |
| `GET /notifications`, `GET /notifications/unread-count`, `POST /notifications/mark-all-read`, `PATCH /notifications/{id}/read`, `DELETE /notifications/{id}` | Read and manage notifications. |
| `GET /fs/list`, `GET /fs/branches` | Inspect supported local filesystem paths and branches. |
| `POST /tasks/{id}/terminals`, `GET /tasks/{id}/terminals`, `GET /tasks/{id}/terminals/availability`, `GET /terminals/{id}`, `POST /terminals/{id}/attach-token`, `POST /terminals/{id}/resize`, `POST /terminals/{id}/terminate`, `GET /terminals/{id}/ws` | Manage a terminal bound to a Task workspace and its exact active lease. |
| `GET /tasks/{id}/external-links`, `POST /tasks/{id}/external-links`, `DELETE /tasks/{id}/external-links/{link_id}` | Manage generic external Task links. |

OAuth endpoints for MCP clients are `/oauth/register`, `/oauth/authorize`,
`/oauth/token`, the `.well-known` metadata routes, and the authenticated
`/oauth/authorize/context` and `/oauth/authorize/approve` routes.

## Historical Release snapshots

`GET /projects/{id}/releases/{release_id}` and
`GET /projects/{id}/milestones/{milestone_id}/releases` read stored immutable
ProjectRelease snapshots and pinned references. They do not recompute
readiness, create or mutate a Release, or authorize current Gate or Task work.
These are `HISTORICAL_READ_ONLY`; physical Release storage remains for PR13.

## Server-Sent Events

`GET /events` emits a bounded public projection. Durable `DomainEvent` records
are the ledger; EventBus notifications only wake the projector. Internal
`ForgeEvent`/`EventContext` variants are never serialized wholesale.

Public durable event types are:

- `task.lifecycle_changed` with exact lifecycle states, versions, cause, and
  optional exact GateEvaluation reference;
- `gate.created`, `gate.policy_revised`, and `gate.evaluated` with exact Gate,
  policy revision, outcome, and input digest facts;
- `execution.started`, `execution.completed`, `execution.failed`,
  `execution.cancelled`, and `execution.stalled`;
- `validation_run.started`, `validation_run.completed`, `evidence.created`,
  and `artifact.created`;
- generic collaboration events `message.created`, `handoff.created`,
  `handoff.status_changed`, `proposal.created`, `proposal.withdrawn`, and
  `decision.recorded`.

Runtime hint names are `project.created`, `project.updated`, `project.deleted`,
`project.paused`, `project.resumed`, `project_hook.run_changed`,
`notification.created`, and `operations.status_changed`.
`events.resync_required` asks clients to refetch active queries after an EventBus
gap. Agent Chat, Attention, old workflow transitions, `task.status`, and
legacy Review-row events are not public event types.

## MCP

The MCP server advertises only registered target tools. The current groups are
Project/Task CRUD, TaskLifecycle receipts, TaskRole membership, Gate and exact
GateEvaluation, explicit Agent Execution and exact-parent follow-up, Review
Execution/ReviewReport, ValidationRun/Evidence, WorkUnit, Agent/profile,
provider-independent Task diff, and generic Message/Handoff/Proposal/Decision
operations. Retired tool names return MCP `-32601 method not found`; they are
not forwarded to a different tool.

`tools/list` is authoritative for the deployed descriptor set. Tools that
start work require an exact Actor or explicit Agent identity and do not infer a
current member or latest Execution.

## Related references

- [CLI reference](cli.md)
- [Architecture](architecture.md)
- [Architecture V2 migration register](migration/architecture-v2.md)
- [Plan PR12 public-surface ledger](migration/plan-pr12-public-surface-alignment.md)
