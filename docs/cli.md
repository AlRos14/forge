# forge-ctl

`forge-ctl` is the CLI client for the Forge REST API. The server must be
running first. By default, `forge-ctl` uses the server from the stored CLI
login, then falls back to the server URL persisted by the last `forge` launch
under the Forge data directory.

## Global flags

```text
--server <URL>            Forge server URL  (default: stored login, then local server)
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
| `task`    | Create, list, show, transition, cancel, archive tasks, preview prompts |
| `agent`   | Register / list / show agents |
| `daemon`  | Link, start, and report an external daemon |
| `run`     | Create + claim a task and follow the SSE stream until terminal state |
| `mcp`     | Helpers for the MCP JSON-RPC endpoint |

Use `forge-ctl <command> --help` for the full set of flags on each subcommand.

## Common flows

### Authenticate the CLI

`forge-ctl login` exchanges your account credentials for a CLI personal access
token and stores it under the Forge data directory. Later commands, including
`forge-ctl mcp install`, reuse that stored token automatically for the same
server URL.

When run in a terminal, `forge-ctl login` prompts for the password without
displaying it. For scripts or piped input, pass `--password-stdin`; an implicit
password prompt fails with guidance when standard input is not a terminal.

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
                      --kind local --local-path /abs/path/to/repo \
                      --default-branch main

forge-ctl agent register --name "Claude" --executor-type shell

forge-ctl task list --project-id <ID>
forge-ctl task show <TASK_ID>
forge-ctl task prompt-preview <TASK_ID> --role coder
forge-ctl task cancel <TASK_ID>
```

`task prompt-preview` is read-only. Add `--trigger accept|reject|fail|retry`
to preview the prompt for a transition target instead of the task's current
state.

### Linking an external daemon

`forge-ctl daemon link` registers the current machine with a running Forge
server, saves daemon credentials, reports installed CLI inventory, keeps
sending heartbeats, and serves filesystem and execution commands over the
daemon command stream. In the web UI: **Daemons → Link daemon** generates the
token and prints the full command:

```bash
forge-ctl daemon link \
  --token fg_... \
  --workspace-root "$HOME/.forge/workspaces"
```

The token is used only for initial ownership; the daemon receives and stores
its own registration token afterward. Add `--once` for a one-shot
registration/report that does not keep the command stream open.
The configured workspace root is created automatically before the daemon
registers or reports.

After a daemon has been linked once, use `forge-ctl daemon start` to run it
again from the saved daemon credentials without registering or claiming it
again:

```bash
forge-ctl daemon start \
  --workspace-root "$HOME/.forge/workspaces"
```

`daemon start` keeps the same heartbeat and command stream open as `daemon
link`. Use `daemon report` only for a one-shot status update; it does not keep
the command stream open.
Forge marks the daemon offline when that command stream disconnects, and uses
stream heartbeats to keep the daemon's last-seen timestamp fresh while it is
connected. When the Forge server starts, external daemons are considered
offline until their command stream reconnects.

Execution dispatch requires the task worktree path created by the server to
exist at the same absolute path on the daemon host. Use a local daemon or mount
the server workspace root into the daemon host/container at the same path. A
daemon on an unrelated filesystem can browse its own `--workspace-root`, but it
cannot run server-created task worktrees yet.

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
