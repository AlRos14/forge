# forge-ctl

`forge-ctl` is the CLI client for the Forge REST API. The server must be
running first (default `http://127.0.0.1:8080`).

## Global flags

```text
--server <URL>            Forge server URL  (default: http://127.0.0.1:8080)
--output <FORMAT>         table | json      (default: table)
```

## Subcommands

| Command | What it does |
|---------|--------------|
| `login`   | Authenticate the CLI and store a reusable token |
| `logout`  | Remove stored CLI credentials |
| `whoami`  | Show stored CLI login state |
| `project` | Create / list / show projects |
| `repo`    | Add / list repos under a project |
| `task`    | Create, list, show, transition, cancel, archive tasks |
| `agent`   | Register / list / show agents |
| `daemon`  | List daemons, **link** an external daemon to a server |
| `run`     | Create + claim a task and follow the SSE stream until terminal state |
| `mcp`     | Helpers for the MCP JSON-RPC endpoint |

Use `forge-ctl <command> --help` for the full set of flags on each subcommand.

## Common flows

### Authenticate the CLI

`forge-ctl login` exchanges your account credentials for a CLI personal access
token and stores it under the Forge data directory. Later commands, including
`forge-ctl mcp install`, reuse that stored token automatically for the same
`--server` URL.

```bash
printf '%s\n' "$FORGE_PASSWORD" | forge-ctl login \
  --email you@example.com \
  --password-stdin

forge-ctl whoami
```

Use `forge-ctl logout` to remove the local credentials file.

### Quick scripted run

```bash
forge-ctl run --project <ID> --repo <ID> --agent <ID> \
              --title "fix login bug" \
              --description "patch the session handler"
# Exits 0 on done; 1 on blocked / merge_failed / cancelled.
```

This creates the task, claims it (which auto-dispatches the executor), then
streams events until the task reaches a terminal state. Useful in CI or shell
pipelines.

### Manual task management

```bash
forge-ctl project create --name "My Project"
forge-ctl repo create --project-id <ID> --name "main-repo" \
                      --url /abs/path/to/repo --default-branch main

forge-ctl agent register --name "Claude" --executor-type shell

forge-ctl task list --project-id <ID>
forge-ctl task show <TASK_ID>
forge-ctl task cancel <TASK_ID>
```

### Linking an external daemon

`forge-ctl daemon link` registers the current machine with a running Forge
server, saves daemon credentials, reports installed CLI inventory, and keeps
sending heartbeats. In the web UI: **Daemons → Link daemon** generates the
token and prints the full command:

```bash
forge-ctl --server http://127.0.0.1:8080 daemon link \
  --token fg_... \
  --workspace-root "$HOME/.forge/workspaces"
```

The token is used only for initial ownership; the daemon receives and stores
its own registration token afterward. Add `--once` for a one-shot
registration/report.

### Installing MCP client config

`forge-ctl mcp install` writes the Forge MCP URL into a supported MCP client
config file. MCP requests require authentication; after `forge-ctl login`, the
stored CLI token is used automatically. You can still pass `--token` or set
`FORGE_TOKEN` to override the stored token:

```bash
forge-ctl mcp install --agent claude
forge-ctl mcp install --agent codex --project-id <PROJECT_ID>
forge-ctl mcp install --agent cursor --scope user --token fg_...
```

Supported agents are `claude`, `codex`, and `cursor`. Supported config scopes
are `project`, `local`, and `user`; the optional `--project-id` scopes MCP tool
calls to one Forge project.

### JSON output for scripting

```bash
forge-ctl --output json task list --project-id <ID> | jq '.items[].title'
```

Every subcommand respects `--output json` and emits the same payload structure
the REST API does — the tables shown in the default mode are just a render of
that JSON.
