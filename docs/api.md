# API Reference

Forge exposes the versioned REST API at `/api/v1`. This reference describes
the post-PR13 public contract for Project, Task,
Actor, TaskRole, RoleMembership, Execution, HarnessSession, WorkUnit,
Workspace, collaboration, Artifact, Review, Validation, Evidence, Gate, and
TaskLifecycle records.

> The orchestrator owns work. The harness owns cognition.

Main/Project Agent, Product Genesis, Agent Chat, Attention, Charter, baseline,
milestone governance, legacy review rows, workflow state, and native runtime
controls are not operational REST resources. PR13 migrations V121–V134 complete
the persistence cutover and provider-start receipt changes. Retired runtime
schemas were removed after exact-history cutovers; preserved legacy records
remain in dedicated immutable history and migration ledgers. Unknown `/api/v1`
paths return the normal API `404 not_found` response;
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

If a provider Start may have taken effect but its response is uncertain,
`POST /tasks/{id}/executions` returns HTTP `409` with code
`execution_start_uncertain`; its message identifies the exact Execution. Read
that Execution and do not submit the Start again automatically.

WorkUnit creation currently has no idempotency key. Validation of PR14's
original head observed `POST /tasks/{task_id}/work-units` return
`500 internal_error` with SQLite code 517 (`database is locked`) after the
WorkUnit row had committed. A deterministic two-connection reproduction
located the failure in `TaskIntegrationOperationRepo::finish`: a deferred
read-then-write transaction could not upgrade after a concurrent commit. The
finish transaction now starts with `BEGIN IMMEDIATE`. If a create response is
ambiguous, inspect `GET /tasks/{task_id}/work-units` before retrying; a blind
retry can create a duplicate.

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
| `GET /repos/{id}`, `PATCH /repos/{id}`, `DELETE /repos/{id}`, `POST /repos/{id}/sync` | Read, update, remove, or sync a repository. `PATCH` can set `pr_provider` and its polling/base URL configuration; setting the provider to `null` removes the provider configuration. Blank token input leaves the saved credential unchanged. Responses expose provider type, token presence, and polling interval without returning the secret. Deleting a Repo with any associated Task returns `409 repo_in_use` and leaves the Repo, primary-Repo selection, and Tasks intact. Deleting a Task-free Repo returns `204` and clears its primary-Repo selection atomically. |
| `GET /projects/{id}/project_hook_runs` | Read generic Project hook run history. |
| `GET /projects/{id}/media`, `POST /projects/{id}/media` | List or upload Project media. |
| `GET /projects/{id}/media/{asset_id}` | Read an exact media asset. |
| `POST /projects/{id}/media/{asset_id}/redact`, `.../purge` | Record the existing media redaction or purge action. |
| `GET /projects/{id}/integration`, `POST /projects/{id}/integration`, `PATCH /projects/{id}/integration`, `POST /projects/{id}/integration/sync` | Manage the Project's external issue integration. Create accepts and GET/PATCH return `credential_env_var`, the environment variable name Forge reads during sync; the credential value is never returned or stored. Optional `default_implementer` is an exact `ActorRef`; imported issues create ordinary Tasks and, when configured, an `implementer` TaskRole with that Actor as a RoleMembership. Sync fails closed if the configured Actor is not valid for the Project. Lifecycle begins at the normal Task creation state; legacy `default_task_state` and singular assignment fields are rejected. |
| `GET /projects/{id}/members`, `POST /projects/{id}/members`, `PATCH /projects/{id}/members/{user_id}`, `DELETE /projects/{id}/members/{user_id}` | Manage Project membership. |

Project reads require the authenticated user to own the Project or have a
`project_member` row; every membership role, including `viewer`, can read, and
ownerless system Projects retain public read behavior. Invisible reads return
the normal `404 not_found` response. Project lists filter by this read rule
before pagination and report counts only for the caller's visible Projects.

Visibility does not grant mutation authority. Ordinary Project-derived work
requires the owner or an explicit `owner`, `admin`, or `member` membership;
the member write rule follows the earlier Task/resource handlers, which used
the same owner-or-membership visibility check for ordinary writes. Project
membership add/update/remove already requires owner/admin in
`ProjectMemberService`. Viewers retain reads but no longer inherit writes from
visibility. Project administration and
sensitive configuration, including Project update/delete/pause/resume, generic
hooks, Repo/PR provider configuration, and integrations, require owner/admin.
Membership administration keeps its existing owner/admin checks. Ownerless
Projects remain readable, but are writable only through an explicit membership
with the role required for that operation; public visibility does not select or
create an owner. Media upload and evidence attachment use ordinary write
authority; redaction and purge keep the existing media owner/admin disposition
check. Foreign private Project reads continue to return `404`; visible callers
without write authority receive `403`.

MCP uses the authenticated `McpContext.user_id` for the same read, write, and
admin rules as REST. `McpContext.project_id` only narrows resource scope. A
Project-scoped call cannot gain authority from its scope, and user/local calls
still authorize each exact resource. Creating a Project through REST or MCP
records the authenticated Human as owner and creates the owner's membership
row.

Repo and integration endpoints resolve the owning Project before access. Task,
Workspace, Execution, dependency, external-link, Review, ValidationRun,
Evidence, Gate, and collaboration endpoints resolve their exact parent Task
and apply the same Project visibility rule before reads or mutations. A known
resource UUID does not grant access to another user's Project. Project-scoped
MCP configuration also checks visibility before reading or writing repo config.
REST and MCP Agent rosters filter private identities before pagination, so
cursors and page metadata do not reveal other users' Agents. Provider runtime
projections include only visible Agents and Daemons.

Project media remains a Project resource. The retired Task comment/media
attachment surface is not part of this API. Ordinary Task creation does not
inherit role assignments from historical Project settings; create exact
TaskRole and RoleMembership records through the Task role endpoints. Legacy
state-driven lifecycle hook settings are preserved as history but are no
longer dispatched. Generic Project hook rules are a separate target surface.

External issue integration secrets are accepted only on create/update and are
never returned. Legacy integration default-state and singular-assignee values
are no longer runtime columns; V123 preserves exact source values as migration
provenance and active defaults use an exact `default_implementer` ActorRef.

## Tasks and TaskLifecycle

| Method and path | Contract |
| --- | --- |
| `POST /projects/{project_id}/tasks` | Create an ordinary Task. Supported fields are `title`, `description`, `parent_task_id`, `task_type`, and `priority`. |
| `GET /projects/{project_id}/tasks` | List Tasks. Filter with `lifecycle_state`, `task_type`, `priority`, and `q`; order with the supported Task sort fields. `status`, `include_cancelled`, and `include_archived` are rejected. |
| `GET /tasks/{id}`, `PATCH /tasks/{id}`, `DELETE /tasks/{id}` | Read, update, or delete a Task. Updates require the Task `version`. |
| `POST /tasks/{id}/subtasks/reorder` | Reorder exact child Task IDs. |
| `GET /tasks/{id}/dependencies`, `POST /tasks/{id}/dependencies`, `DELETE /tasks/{id}/dependencies/{dep_id}`, `GET /tasks/{id}/dependents` | Read and update Task dependencies. |
| `GET /tasks/{id}/workspace`, `GET /tasks/{id}/diff` | Read the Task integration workspace or inspect its current diff. Workspace mutation and recovery require an exact WorkUnit and Workspace identity. |
| `GET /tasks/{id}/lifecycle` | Read the authoritative aggregate TaskLifecycle and its version. |
| `POST /tasks/{id}/lifecycle` | Request a deterministic TaskLifecycle edge with `to_state`, `expected_lifecycle_version`, and `idempotency_key`; use `gate_evaluation_id` for an edge admitted by that exact evaluation. `reason_kind` and `reason_ref` must be supplied together, with non-empty values, or both omitted. |
| `GET /tasks/{id}/lifecycle/transitions` | Read exact durable transition receipts and facts. |
| `GET /tasks/{id}/gates`, `POST /tasks/{id}/gates` | List or create Task-scoped Gates. |
| `GET /gates/{id}`, `PUT /gates/{id}/policy`, `POST /gates/{id}/evaluate` | Read a Gate, revise its policy against the expected revision, or evaluate its exact current inputs. |
| `GET /gate-evaluations/{id}` | Read one immutable GateEvaluation and its frozen input facts. |
| `POST /tasks/{id}/merge` | Request merge admission using the exact satisfied GateEvaluation. The service rechecks its current policy and input identities under the Task integration lock. |
| `GET /tasks/{id}/task-roles`, `POST /tasks/{id}/task-roles`, `PATCH /tasks/{id}/task-roles/{role}` | Read or update TaskRole definitions. |
| `POST /tasks/{id}/task-roles/{role}/members`, `PATCH /tasks/{id}/task-roles/{role}/members/{membership_id}` | Add or update an exact RoleMembership. Memberships name a Human or Agent Actor. |

When present, `parent_task_id` must identify a root Task in the same Project;
the parent and child `repo_id` values must match, including when both are null.
Only one subtask level is supported, and a Task with children cannot itself
become a subtask. TaskService enforces parent-backed create,
batch create, attach, reparent, detach, and Task soft-delete in one SQLite
`BEGIN IMMEDIATE` transaction. A Task with live subtasks cannot be deleted;
move, detach, or delete those subtasks first. New attachments and reparents
append at the next destination `subtask_order`; detach clears both fields. An
ordinary Task update leaves the existing structure untouched.

TaskLifecycle is aggregate progress and the sole persisted Task state
authority. V129 removed `task.status`, task state configuration, and entry
barrier compatibility columns after archiving their exact values. Workflow
templates, transition logs, and legacy GateConfig do not advance TaskLifecycle;
their source records remain only in migration history where preserved.

## Executions and HarnessSessions

| Method and path | Contract |
| --- | --- |
| `GET /tasks/{id}/executions` | List exact historical Task Executions. |
| `POST /tasks/{id}/executions` | Start an Agent Execution from `work_unit_id` and `expected_work_unit_version`, with explicit `purpose`, `prompt`, and optional exact `input_artifact_ids` and overrides. Role and Actor come from the allocated WorkUnit. A repository WorkUnit that requires integration prepares and binds its exact isolated Workspace; supported non-repository WorkUnits have no Workspace. |
| `POST /tasks/{task_id}/executions/{execution_id}/recovery` | Explicitly select one cancelled exact crash-recovery, Agent-timeout, or graceful-shutdown Execution for a fresh child attempt. Supply its exact `work_unit_id`, expected Task, TaskLifecycle, and WorkUnit versions, and an `idempotency_key`; an optional `context` is included in the request identity. The selected Execution's Agent must still be the WorkUnit's current assignee and active role member. The requesting Human must be authorized to mutate the Task and is recorded in the decision receipt. A same-key retry returns the exact admitted child without dispatching it again. Selecting one candidate from an ambiguous aggregate block resolves that block and admits the child in one transaction; remaining WorkUnits keep their own pending Execution history and can be selected afterward. This human selection may explicitly override an Auto candidate when the operator chooses a fresh attempt; dispatcher Auto recovery itself resumes only the parent's exact reusable HarnessSession. |
| `GET /executions/{id}` | Read one exact Execution, including its Actor, role, purpose, HarnessSession reference, frozen execution configuration, and workspace identity. |
| `POST /executions/{id}/follow-up` | Create a child from the exact parent Execution. `idempotency_key`, `message`, and `agent_id` are required. Repeating the same key and request replays the exact child; reusing the key with different request content conflicts. A new authorized follow-up uses a new key. A same-Agent continuation can reuse only the exact compatible HarnessSession; it never substitutes a current role member or latest Execution. |
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
| `POST /tasks/{id}/review-executions` | Start a Human Review Execution. `workspace_id`, when supplied, must name the exact Ready Workspace to review; `null` starts a Workspace-free review and never selects a Task Workspace implicitly. The response names that exact Execution. |
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
AgentAction, the historical Project readiness workflow, or Main/Project Agent
workflows. WorkUnit readiness is a current deterministic projection.

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
| `GET /notifications`, `GET /notifications/unread-count`, `POST /notifications/mark-all-read`, `PATCH /notifications/{id}/read`, `DELETE /notifications/{id}` | Read and manage notifications for visible Projects. Unscoped lists/counts and mark-all-read operate only on the caller's visible Project set; an explicit invisible Project or another user's notification returns `404`. |
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
These are `HISTORICAL_READ_ONLY` and use the normal Project owner/member
visibility rule, including public ownerless system Projects. The reader uses
`project_release` plus self-contained `project_release_history_*` snapshots,
shared media assets, and tombstones; it does not query Project OS tables or
recompute a snapshot.

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

SSE requires authentication and filters Project- and Task-scoped events through
the same read visibility rule as REST. Before deleting a Project, Forge captures
the exact owner and Project member recipients. After teardown it sends a
per-user `project.deleted` invalidation hint to that snapshot. The public payload
contains no recipient list, outsider streams receive no hint, and recipient
authorization does not query the deleted Project. Operations status hints are
limited to administrators; resync hints carry no Project data.

## MCP

The MCP server advertises only registered tools. The current tool names are:

- Projects: `forge_create_project`, `forge_list_projects`, `forge_get_project`,
  `forge_update_project`, `forge_update_project_hooks`.
- Tasks and lifecycle: `forge_create_task`, `forge_create_sub_tasks`,
  `forge_list_tasks`, `forge_get_task`, `forge_update_task`,
  `forge_get_task_diff`, `forge_get_task_lifecycle`,
  `forge_list_task_lifecycle_transitions`, `forge_transition_task_lifecycle`,
  `forge_add_task_dependency`, `forge_remove_task_dependency`,
  `forge_list_task_dependencies`.
- Executions and roles: `forge_list_executions`, `forge_start_execution`,
  `forge_follow_up_execution`, `forge_list_task_roles`,
  `forge_create_task_role`, `forge_add_task_role_member`.
- Gates and evidence: `forge_create_task_gate`, `forge_get_gate`,
  `forge_revise_gate_policy`, `forge_evaluate_gate`,
  `forge_get_gate_evaluation`, `forge_list_task_review_executions`,
  `forge_get_review_execution`, `forge_list_validation_runs`,
  `forge_get_validation_run`, `forge_get_evidence`,
  `forge_list_task_artifacts`.
- Agents and collaboration: `forge_register_agent`, `forge_list_agents`,
  `forge_list_agent_profiles`, `forge_list_task_messages`,
  `forge_list_task_handoffs`, `forge_list_task_proposals`,
  `forge_list_task_decisions`.

`forge_start_execution` requires the exact WorkUnit ID and version, Purpose,
and prompt. The server resolves the matching Workspace when the WorkUnit needs
repository integration; non-repository WorkUnits have no Workspace. The
descriptor schemas are returned by MCP `tools/list` and are implemented in
`crates/mcp-server/src/tools/descriptors.rs`. Retired tool names return MCP
`-32601 method not found`; they are not forwarded to a different tool.

`tools/list` is authoritative for the deployed descriptor set. Tools that
start work require an exact Actor or explicit Agent identity and do not infer a
current member or latest Execution. In user/local mode, Project lists contain
only visible Projects and Task, Execution, Gate, Review, ValidationRun,
Evidence, Artifact, and collaboration tools resolve their exact parent Task
and Project before reading or mutating. Project-scoped connections reject
cross-Project IDs even when the user can access that other Project separately.
Agent lists contain global and caller-owned identities with visibility applied
before pagination; MCP registration binds the new Agent to the authenticated
Human. Only admins may pin an Agent to a daemon, and non-admin Agent projections
omit daemon IDs and redact sensitive configuration values.

An uncertain provider Start returns MCP error `-32029` with data code
`execution_start_uncertain` and the exact `execution_id`. Clients must not
retry that Start automatically because the provider may already have acted.

`forge_update_task` accepts an optional `parent_task_id`; omitting the field
preserves the current parent, while explicit `null` detaches the Task. Both
forms use the same TaskService structural transaction as REST.

## Related references

- [CLI reference](cli.md)
- [Architecture](architecture.md)
- [Architecture V2 migration register](migration/architecture-v2.md)
- [Plan PR12 public-surface ledger](migration/plan-pr12-public-surface-alignment.md)
