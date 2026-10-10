# forge-ctl

`forge-ctl` is the client for Forge's REST API. The server must be running.
It uses an explicit `--server` URL first, then the saved CLI login, then the
server URL recorded by the last local `forge` launch.

## Global options

```text
--server <URL>       Forge server URL
--output <FORMAT>    table | json (default: table)
```

## Commands

| Command | Purpose |
| --- | --- |
| `login`, `logout`, `whoami` | Manage the CLI's user token. |
| `project create`, `project list` | Create and list Projects. |
| `repo create`, `repo list` | Register and list Project repositories. |
| `task create`, `task list`, `task get`, `task role`, `task transition`, `task execute` | Manage Tasks, TaskRole membership, TaskLifecycle, and explicitly scoped Executions. |
| `agent register`, `agent list`, `agent get`, `agent profile` | Inspect external Harness Agents and select immutable profiles. |
| `provider` | Manage provider credentials and authorization. |
| `daemon link`, `daemon start`, `daemon report` | Register and report an external execution daemon. |
| `mcp install`, `mcp uninstall`, `mcp status` | Configure Forge as an MCP server in a supported client. |

Use `forge-ctl <command> --help` for argument details.

## Authentication

`forge-ctl login` exchanges account credentials for a personal access token and
stores it under the Forge data directory. In a terminal it prompts without
echoing the password. For scripts, pass `--password-stdin`:

```bash
printf '%s\n' "$FORGE_PASSWORD" | forge-ctl login \
  --email you@example.com --password-stdin
forge-ctl whoami
```

Use `forge-ctl logout` to remove the local token. Commands accept `--output
json` for scripting.

## Project, repository, and Task workflow

Project creation is ordinary Project creation; it does not configure a Main
Agent, Project Agent, Charter, Genesis session, or execution baseline.

```bash
forge-ctl project create --name "My Project"
forge-ctl project list

forge-ctl repo create --project-id <PROJECT_ID> --name main \
  --kind local --local-path /abs/path/to/repo --default-branch main
forge-ctl repo list --project-id <PROJECT_ID>

forge-ctl task create --project-id <PROJECT_ID> --title "Fix login"
forge-ctl task list --project-id <PROJECT_ID> --lifecycle-state active
forge-ctl task get <TASK_ID>
forge-ctl task role create <TASK_ID> --role implementer --coordination-mode independent
forge-ctl task role add-member <TASK_ID> implementer --actor-kind agent --actor-id <AGENT_ID>
forge-ctl task role list <TASK_ID>
forge-ctl task execute <TASK_ID> --agent-id <AGENT_ID> \
  --role implementer --purpose implement --prompt "Fix the login error"
forge-ctl task transition <TASK_ID> blocked \
  --expected-lifecycle-version 3 --idempotency-key <KEY> \
  --reason-kind dependency --reason-ref issue:123
```

Task lists filter with `--lifecycle-state` and display TaskLifecycle. A
transition is versioned and idempotent. A transition to `ready-to-merge`
requires the exact satisfied Gate evaluation:

```bash
forge-ctl task transition <TASK_ID> ready-to-merge \
  --expected-lifecycle-version 4 --idempotency-key <KEY> \
  --gate-evaluation-id <GATE_EVALUATION_ID>
```

Gate evaluation and merge admission are also available through the REST API.
`task execute` requires the exact Agent, active TaskRole membership, purpose,
and prompt. It never selects a current role member implicitly. Tasks with
WorkUnits fail closed until an explicit WorkUnit-scoped Execution command is
available. The CLI will not translate old Task status or workflow values.

## Agents and provider credentials

Agents are Harness-bound participants. Register an external Harness Agent and
inspect its profiles with:

```bash
forge-ctl agent register --name "Build worker" --executor-type shell
forge-ctl agent list
forge-ctl agent profile list <AGENT_ID>
forge-ctl agent profile select <AGENT_ID> <PROFILE_ID> --version <VERSION>
```

Provider credentials live under the separate `provider` command. API keys are
read from a hidden terminal prompt or from stdin; credential values are never
printed:

```bash
forge-ctl provider add --provider openai --label work
forge-ctl provider login --provider openai --label chatgpt
forge-ctl provider list
forge-ctl provider rename <ENTRY_ID> --label team --version <VERSION>
forge-ctl provider remove <ENTRY_ID> --version <VERSION>
```

Use `--credential-stdin` for non-interactive API key input. OAuth login supports
browser and device flows. Provider entries do not create Agents or Harness
sessions.

## External daemon

`daemon link` registers the machine and keeps the heartbeat and command stream
open. The token establishes ownership during the first registration; the
daemon stores its own credentials afterward. Add `--once` to register and
report once. Later, `daemon start` uses saved daemon credentials and keeps the
stream open; `daemon report` sends one report.

```bash
forge-ctl daemon link --token fg_... \
  --workspace-root "$HOME/.forge/workspaces"
forge-ctl daemon start --workspace-root "$HOME/.forge/workspaces"
```

The Task worktree must exist at the same absolute path on the execution host.
Use a local daemon or mount the server workspace root at that path.

## MCP client configuration

MCP requests use the stored login token unless `--token` or `FORGE_TOKEN`
overrides it. Forge can scope tool access to one Project:

```bash
forge-ctl mcp install --agent claude --project-id <PROJECT_ID>
forge-ctl mcp install --agent codex --scope user
forge-ctl mcp status --agent cursor --scope project
forge-ctl mcp uninstall --agent claude --scope project
```

Supported clients are `claude`, `codex`, and `cursor`; scopes are `project`,
`local`, and `user`. MCP exposes the same target domain: TaskLifecycle,
TaskRoles, Gates, exact Review Executions, ValidationRuns, Evidence, and generic
collaboration records. Retired tools are not announced or translated.
