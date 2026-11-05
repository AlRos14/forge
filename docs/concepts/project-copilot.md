# Project Copilot

A Project Copilot is an existing harness-bound Agent assigned to help the
Project's Humans organize work. The Copilot panel remains available while
navigating the Project, including a Task detail page. The manual Task and
WorkUnit controls remain available alongside it.

Owners and Project admins assign or replace the Copilot. The Agent must belong
to the canonical Project owner's account and be pinned to the supported local
embedded host. Global Agent discovery grants no cross-account execution
consent. Project members may send instructions; viewers may inspect the
conversation without starting a turn.

## Using the panel

Open a Project or Task and select **Copilot** in the bottom-right corner. An
owner or Project admin can select an eligible Agent in the panel. The list
includes harnesses reported by the server as supported on the online embedded
host, and only Agents owned by the Project owner and pinned to that host. If
there is no eligible Agent, open **Agent settings** from the panel; a server
administrator can bind the owner's Agent to an available host there. Project
Copilot currently runs only on Forge's local embedded host, not remote daemons.
The panel stays beside the page on desktop and opens as a modal on mobile.

From a Task, ask for a proposed breakdown or delegate creation explicitly, for
example: "Create WorkUnits for this Task with dependencies and eligible
workers." Existing TaskRoles supply the available responsibilities and members.
The conversation shows the response; Activity shows recorded tool outcomes and
WorkUnit changes. Clear the attached Task context for Project-wide management.

## Conversation and delegation

Describe the desired outcome in the panel. A request to propose WorkUnits
produces a proposal in the conversation. An instruction to create or organize
them delegates that management operation. A small Task may need one WorkUnit;
larger Tasks may need several scopes and explicit dependencies. The Copilot
uses the Task brief and current Project data, preserving the requested scope
and its restrictions.

Opening the panel from a Task attaches that exact Task ID. The server checks
that it belongs to the same Project and fences Task operations to that exact
Task for the turn. Clear the Task context to manage multiple Tasks within the
Project. Deleting or moving the attached Task revokes its run rather than
broadening its authority. This does not create TaskRole
membership, a WorkUnit assignment, an Execution, or a WorkspaceLease.

Messages and management runs are durable Project records. Each run identifies
the exact Agent, requesting Human, Project binding version, optional context
Task, and terminal outcome. Only one turn runs at a time in a Project.
Idempotency keys distinguish retrying the same send from sending a new
instruction. An interrupted run is recorded as interrupted rather than
automatically replayed: it may already have performed management actions.

## Harness boundary

The harness owns cognition. Forge starts the configured Agent in a dedicated
Project Copilot context and supplies the conversation plus explicit current
Task context. This Project management run is separate from a Task Execution;
it creates no placeholder Task or WorkUnit. Native sessions are never inferred
from a role or the most recent Execution.

The API derives `supported_executor_types` from the registered adapters and
the embedded daemon's current authenticated CLI report. It also returns the
exact `supported_daemon_id`. The panel offers only Agents owned by the Project
owner and pinned to that daemon. Each advertised adapter must install a
server-owned per-run MCP connection and prove that only the scoped Project
Copilot tools are callable; adapters without that policy are not advertised
and fail closed before a model request. Remote daemons are unsupported.

The selected Agent configuration supplies the model, reasoning effort and
prompt template. Project Copilot rejects custom executables, extra CLI
arguments, Agent environment overrides, fallback routes and Claude Code Router.
Each adapter applies its native per-run policy to disable unrelated native
tools and allow only the run's Forge MCP tools. Claude Code disables built-in
tools with `--tools ""`, allows only the `mcp__forge_project__*` namespace and
loads a strict per-run MCP file. The MCP bearer credential exists only in the
child environment; it is not written to configuration files or command
arguments. Managed host policy continues to apply.

Forge injects its MCP connection through native per-run harness configuration.
A short-lived credential is limited to the live run and Project. Only its hash
is stored. The credential is delivered in the subprocess environment; URLs,
command arguments, saved MCP settings and public conversation responses do
not contain it. Completion, cancellation, replacement of the Copilot, loss of
membership or ownership, and expiry revoke the delegation.

## Management authority and activity

The initial MCP delegation supports reading Project/Task context and managing
WorkUnit scopes, allocations and dependencies. TaskRoles and their current
members remain the eligibility authority. WorkUnit creation grants no
execution or repository authority. Worker start, TaskLifecycle transitions,
Gates, integration, merge, account administration and credential management
remain outside this delegation.

Each WorkUnit mutation uses the real Agent Actor and correlates its durable
domain event with the Copilot run. The requesting Human remains part of the
run's provenance. Current delegation is checked at authentication and service
boundaries, and again within the transaction that commits the mutation event.
Project scope cannot be widened using another Project ID in MCP arguments.

The new Project Copilot contract does not reactivate historical Project Agent,
Agent Chat or Project OS records. Their archived data retains its historical
meaning. See the [Project Copilot validation record](../project-copilot-validation.md)
for the focused checks and remaining live acceptance limits.
