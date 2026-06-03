# Getting Started

This guide takes you from a blank machine to a real task driven through `todo → done`
against your own git repo.

## Install

### npm bootstrapper (macOS / Linux)

```bash
npx @forgeailab/forge --demo
```

The npm package is a small bootstrapper. It downloads the matching Forge GitHub
release archive for macOS, glibc Linux, or musl Linux, caches it under
`~/.forge/npx`, and starts the local server with the bundled web UI assets. The
browser does not open automatically; pass `--open` to opt in.

### Homebrew (macOS / Linux, recommended)

```bash
brew install forgeailab/tap/forge
```

The tap repo is [`ForgeAILab/homebrew-tap`](https://github.com/ForgeAILab/homebrew-tap).
The formula installs both `forge` and `forge-ctl` and places the web UI assets under
the Homebrew `share/forge` prefix.

### Install script (curl)

```bash
curl -fsSL https://raw.githubusercontent.com/ForgeAILab/forge/main/install.sh | bash
```

Or grab a tarball directly from [Releases](https://github.com/ForgeAILab/forge/releases).
Archives ship `forge`, `forge-ctl`, and the built web UI assets. The installer puts
the UI under `/usr/local/share/forge/web/dist` and selects the musl Linux archive
on musl-based systems such as Alpine. For a manual install, run `forge` from the
extracted archive root or set `FORGE_WEB_DIST_DIR` to the extracted `web/dist`
directory.

### Build from source

```bash
git clone https://github.com/ForgeAILab/forge.git
cd forge
cargo build
cargo run -p forge-cli         # plain start, data in ~/.forge/
cargo run -p forge-cli -- --demo  # seed labelled demo data (idempotent)
```

### Docker

```bash
docker compose up -d
# Forge available at http://localhost:8080
```

Data persists in the `forge-data` Docker volume. Set `RUST_LOG=debug` in
`docker-compose.yml` for verbose output.

## First boot

By default the server:

- Binds loopback on an OS-selected port the first time, then reuses that port
  from `~/.forge/server.json` on later starts.
- Creates `~/.forge/forge.db` (SQLite, WAL mode).
- Boots an embedded daemon that auto-registers and reports installed CLIs
  (`shell` always, plus `codex` / `claude_code` / `cursor` / `gemini` /
  `opencode` when on `PATH`).
- Upserts default executor profiles from the adapter registry.

Open the `management_url` printed in the server logs for the web UI. For raw
API calls, set:

```bash
FORGE_URL=$(jq -r .server_url ~/.forge/server.json)
```

## Configuration

Precedence: **CLI flags > env vars > config file > defaults**.

```bash
cargo run -p forge-cli                          # plain start
cargo run -p forge-cli -- --demo                # seed demo data
cargo run -p forge-cli -- --no-embedded-daemon  # external daemon mode
cargo run -p forge-cli -- --no-mcp              # disable MCP endpoint
FORGE_DATA_DIR=./test cargo run -p forge-cli    # override data dir via env
```

Useful env vars: `FORGE_DATA_DIR`, `FORGE_WORKSPACE_ROOT`,
`FORGE_WORKSPACE_CLEANUP_DELAY_SECONDS`, `FORGE_WEB_DIST_DIR`, `RUST_LOG`.

### Local development data dir

`make dev` and friends point data at `./test/` (gitignored) so dev state never
pollutes `~/.forge`. See the project [Makefile](../Makefile).

## Configuring agents

The embedded daemon auto-detects installed CLIs. Verify what's available:

```bash
curl -sS "$FORGE_URL/api/v1/daemons" | jq '.items[].cli_inventory'
```

Register an agent against one of the reported CLIs:

```bash
curl -sS -X POST "$FORGE_URL/api/v1/agents" \
  -H 'content-type: application/json' \
  -d '{
    "name": "claude-coder",
    "executor_type": "claude_code",
    "daemon_id": "<daemon-id-from-above>"
  }'
```

For Cursor, use `"executor_type": "cursor"`. Forge runs `cursor-agent` in
headless print mode with stream JSON output; set `CURSOR_API_KEY` or run
`cursor-agent login` first so the daemon reports it as authenticated.

The `shell` executor is always available and useful for scripted tests — see the
walkthrough below.

## End-to-end walkthrough

This drives a task from `todo → done` against a real local repo, using the
`shell` executor so you don't need any AI CLI installed.

```bash
# 1. Create a project + repo pointing at a real git checkout.
PROJECT_ID=$(curl -sS -X POST "$FORGE_URL/api/v1/projects" \
  -H 'content-type: application/json' \
  -d '{"name":"demo"}' | jq -r .id)

curl -sS -X POST "$FORGE_URL/api/v1/projects/$PROJECT_ID/repos" \
  -H 'content-type: application/json' \
  -d '{"name":"my-repo","url":"/abs/path/to/repo","default_branch":"main"}'

# 2. Use the auto-reported daemon and register a shell agent.
DAEMON_ID=$(curl -sS "$FORGE_URL/api/v1/daemons" | jq -r '.items[0].id')
AGENT_ID=$(curl -sS -X POST "$FORGE_URL/api/v1/agents" \
  -H 'content-type: application/json' \
  -d "{\"name\":\"demo-agent\",\"executor_type\":\"shell\",\"daemon_id\":\"$DAEMON_ID\"}" \
  | jq -r .id)

# 3. Create a task with inline CI steps.
TASK_ID=$(curl -sS -X POST "$FORGE_URL/api/v1/projects/$PROJECT_ID/tasks" \
  -H 'content-type: application/json' \
  -d '{
    "title":"greet",
    "description":"echo hi > greeting.txt && git add . && git -c user.email=a@b -c user.name=a commit -m hi",
    "review_config":{"ci_steps":["test -f greeting.txt"]}
  }' | jq -r .id)

# 4. Claim the task — the executor auto-dispatches.
curl -sS -X POST "$FORGE_URL/api/v1/tasks/$TASK_ID/claim" \
  -H 'content-type: application/json' \
  -d "{\"agent_id\":\"$AGENT_ID\",\"overrides\":null}"

# 5. Transition to review. The review runner fires the CI steps inline and
#    returns {task, review} in one response.
curl -sS -X POST "$FORGE_URL/api/v1/tasks/$TASK_ID/transition" \
  -H 'content-type: application/json' \
  -d '{"status":"review","version":2}'

# 6. Transition to merging. The merge runs, the task auto-advances to done,
#    and the worktree is cleaned up synchronously.
curl -sS -X POST "$FORGE_URL/api/v1/tasks/$TASK_ID/transition" \
  -H 'content-type: application/json' \
  -d '{"status":"merging","version":3}'
```

The same flow is exercised end-to-end by `cargo test -p api --test happy_path`.

## Using `forge-ctl`

For interactive work, the CLI is friendlier than raw curl:

```bash
printf '%s\n' "$FORGE_PASSWORD" | forge-ctl login \
  --email you@example.com \
  --password-stdin

forge-ctl project create --name "My Project"
forge-ctl task list --project-id <ID>
forge-ctl agent register --name "Claude" --executor-type shell

# Create a task, claim it, follow the SSE stream until terminal state:
forge-ctl run --project <ID> --repo <ID> --agent <ID> \
              --title "fix login bug" \
              --description "patch the session handler"
# Exits 0 on done; 1 on blocked / merge_failed / cancelled.
```

Full CLI reference → [docs/cli.md](cli.md).

## Linking an external daemon

`forge-ctl daemon link` registers the current machine with a running Forge
server, saves daemon credentials, reports local CLI availability, and keeps
sending heartbeats. While it is running, it also keeps the daemon command
stream open so Forge can browse local paths and dispatch agents on that
machine. In the web UI: **Daemons → Link daemon** generates a token and prints
the full command:

```bash
forge-ctl daemon link \
  --token fg_... \
  --workspace-root "$HOME/.forge/workspaces"
```

The token is used only for initial ownership; the daemon receives and stores its
own registration token afterward. Use `--once` for a one-shot
registration/report only; `--once` does not keep the command stream open for
filesystem browsing or execution dispatch.

After the first link, restart the daemon from its saved credentials with:

```bash
forge-ctl daemon start \
  --workspace-root "$HOME/.forge/workspaces"
```

`daemon start` does not register or claim the daemon again; it just reports
local CLI availability and keeps the command stream open. `daemon link` and
`daemon start` create the configured workspace root if it does not already
exist, so filesystem browsing can open the launch directory immediately.

Execution dispatch expects the server-created task worktree to exist at the same
absolute path on the daemon host. For containers, mount the server workspace
root into the container at that same path. A daemon on an unrelated filesystem
can still serve filesystem browsing under its own `--workspace-root`, but it
cannot run server-created task worktrees yet.

## Where to next

- **API surface** → [api.md](api.md)
- **How it's wired together** → [architecture.md](architecture.md)
- **Run agents from your AI tooling** → [api.md#mcp-tools](api.md#mcp-tools)
- **Contribute** → [../CONTRIBUTING.md](../CONTRIBUTING.md)
