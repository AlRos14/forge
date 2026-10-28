# forge-ctl reference

`forge-ctl` is the command-line client for a running Forge REST server.

## Server selection and login

The global option `--server <URL>` selects the server. Without it, the client uses the saved login server, then the address in `<FORGE_DATA_DIR>/server.json` (or `~/.forge/server.json` by default). `FORGE_DATA_DIR` also selects the client credential directory.

The default output is a table. Use the top-level option `--output json` for structured output. The `--server` option is global.

Login stores a personal access token in `<data-dir>/forge_ctl_credentials.json`. For scripts, pass the password through stdin:

```bash
printf '%s\n' "$FORGE_PASSWORD" | forge-ctl login \
  --email you@example.com --password-stdin
forge-ctl whoami
```

Use `forge-ctl logout` to remove the saved token. Avoid putting passwords or provider keys directly in shell history.

## Commands

| Command group | Available operations |
|---|---|
| `login`, `logout`, `whoami` | Authenticate and inspect the saved login. |
| `project` | `create`, `list`. |
| `repo` | `create`, `list`. |
| `task` | `create`, `list`, `get`, `role list/create/add-member`, `execute`, `transition`. |
| `agent` | `register`, `list`, `get`, `profile list/select`. |
| `provider` | `list`, `add`, `login`, `rename`, `remove`. |
| `daemon` | `link`, `start`, `report`. |
| `mcp` | `install`, `uninstall`, `status` for Claude, Codex, and Cursor. |

Use `forge-ctl <command> --help` for the arguments and enum values of that command. Repository creation accepts a local path or a remote URL; the selected source kind must match the supplied path or URL. REST daemon listing and Agent-to-daemon pinning require an administrator.

## Task and Execution workflow

The CLI creates a Project, Repo, Task, TaskRole, and RoleMembership. It does not currently create a WorkUnit. Create and allocate the WorkUnit through the REST or MCP API before starting an Execution.

The CLI accepts only the current TaskLifecycle states: `backlog`, `ready`, `active`, `blocked`, `ready-to-merge`, `merging`, `done`, and `cancelled`. A transition includes the current lifecycle version and an idempotency key. A transition to `ready-to-merge` also needs the exact satisfied GateEvaluation.

`task execute` requires all of the following:

- the Task ID;
- the exact WorkUnit ID and current WorkUnit version;
- the Execution Purpose and prompt.

The Actor and Role come from that WorkUnit's allocation. The server prepares and binds the required repository Workspace. A non-repository WorkUnit may run without a Workspace within the supported local host constraints. The CLI does not select an implicit Task Agent or infer a Workspace.

If a provider Start returns an uncertain outcome, inspect the exact Execution and follow the recovery contract. Do not automatically submit another Start.

## Agents, providers, and daemons

Agents are harness-bound Actors. Registration and profile selection are separate commands. The initial profile is selected when the Agent is created; later profile selection uses the Agent's current version. Supported harnesses and profiles depend on the online host and detected providers.

Provider entries store a credential reference and metadata; secret values are not returned by provider listing. Use `provider add` for API-key entries or `provider login` for supported OAuth flows. A provider entry does not create an Agent or HarnessSession.

`daemon link` registers a host and, unless `--once` is supplied, maintains its report and command stream. `daemon start` reuses saved daemon credentials; `daemon report` sends one report.

## MCP client configuration

`forge-ctl mcp install` writes the selected client's local configuration and points it at the Forge MCP endpoint. The supported client names are `claude`, `codex`, and `cursor`; scopes are `project`, `local`, and `user`. A token is required. It is written as an `Authorization: Bearer ...` header (`headers` for Claude/Cursor JSON and `http_headers` for Codex TOML), never as a URL parameter. The configuration file therefore contains a secret: keep its normal file permissions and do not commit it. A Project-scoped install can restrict tools to one Project.

MCP no longer accepts `?token=` credentials. Existing configurations that contain a tokenized URL must be reinstalled; the web MCP configuration view reports such a legacy URL as not usable. OAuth-capable clients can use the `/oauth` and `/.well-known` flows with a bare `/mcp` URL instead.

The active tool names and schemas are exposed by MCP `tools/list`. REST and MCP use the same domain authority. Neither surface restores retired Main/Project Agent, Agent Chat, Project OS, or legacy WorkflowEngine operations.
