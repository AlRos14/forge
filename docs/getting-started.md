# Getting Started

This guide takes you from a blank machine to an explicitly scoped Task Execution
against your own git repo. TaskLifecycle records aggregate Task progress; an
Execution records one Actor, Role, Purpose, and Harness identity.

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
  `opencode` / `smith` when on `PATH`).
- Upserts default executor profiles from the adapter registry.

Open the `management_url` printed in the server logs for the web UI. For raw
API calls, set:

```bash
FORGE_URL=$(jq -r .server_url ~/.forge/server.json)
```

## Configuration

Precedence: **CLI flags > env vars > config file > defaults**.
Unknown top-level config fields fail parsing; retired `public_search` settings
must be removed from `forge.yaml`.

```bash
cargo run -p forge-cli                          # plain start
cargo run -p forge-cli -- --demo                # seed demo data
cargo run -p forge-cli -- --no-embedded-daemon  # external daemon mode
cargo run -p forge-cli -- --no-mcp              # disable MCP endpoint
FORGE_DATA_DIR=./test cargo run -p forge-cli    # override data dir via env
```

Useful env vars: `FORGE_DATA_DIR`, `FORGE_WORKSPACE_ROOT`,
`FORGE_WORKSPACE_CLEANUP_DELAY_SECONDS`, `FORGE_WEB_DIST_DIR`, `RUST_LOG`.

JWT signing uses `server.jwt_secret` in the config file or `FORGE_JWT_SECRET`
when set. Otherwise Forge reads or creates `<data_dir>/jwt_secret.bin` on first
start (mode `0600` on Unix). Set an explicit secret in production deployments.

### Local development data dir

`make dev` and friends point data at `./test/` (gitignored) so dev state never
pollutes `~/.forge`. See the project [Makefile](../Makefile).

## Registering an external Harness Agent

The embedded daemon auto-detects installed CLIs. Verify what's available:

```bash
curl -sS "$FORGE_URL/api/v1/daemons" | jq '.items[].cli_inventory'
```

Register an Agent identity for an installed Harness:

```bash
forge-ctl agent register --name "Build worker" \
  --executor-type claude_code --daemon-id <DAEMON_ID>
forge-ctl agent list
forge-ctl agent profile list <AGENT_ID>
```

For Cursor, use `--executor-type cursor`. Register an exact Agent profile when a
Task Execution needs that Harness configuration. Agent identity alone grants no
TaskRole membership, WorkspaceLease, or Execution.

Provider credentials and external Harness Agents are separate records. A
provider entry can supply dispatch credentials to a supported external Harness;
it does not create an Agent, select a profile, or grant Task authority. Use the
CLI to manage credentials and identities:

```bash
forge-ctl provider add --provider openai --label work
forge-ctl provider login --provider openai --label chatgpt
forge-ctl provider list
forge-ctl agent register --name "Build worker" --executor-type shell
```

Pass `--credential-stdin` to `provider add` for non-interactive API key input.
For a supported Harness, pass its exact provider entry ID as `--credential-id`
to `agent register`. Provider entries do not create an Agent or HarnessSession.
Use exact TaskRole/RoleMembership records to admit Actors to a Task. See [the
CLI reference](cli.md) for the full provider and Agent command contract.

## Creating a Project and starting scoped work

Project creation is ordinary. It creates no Main Agent, Project Agent, Genesis
session, Charter, baseline, or synthetic Task. A Task records aggregate
TaskLifecycle; each Execution records one exact Actor, TaskRole, Purpose, and
Harness identity. Review is an Execution whose Role is `reviewer` and Purpose is
`review`; ValidationRuns and Evidence keep their own exact identities.

The target-domain walkthrough creates a Project, registers a repository,
creates a Task and an external Harness Agent, then admits that Agent to an
explicit TaskRole before starting an Execution:

```bash
forge-ctl login --email you@example.com --password-stdin <<<"$FORGE_PASSWORD"

# Use --output json before the command and copy each returned ID.
forge-ctl --output json project create --name "Demo"
forge-ctl repo create --project-id <PROJECT_ID> --name main \
  --kind local --local-path /abs/path/to/repo --default-branch main
forge-ctl agent register --name "Build worker" --executor-type shell
forge-ctl task create --project-id <PROJECT_ID> --title "Write greeting"

forge-ctl task role create <TASK_ID> --role implementer \
  --coordination-mode independent
forge-ctl task role add-member <TASK_ID> implementer \
  --actor-kind agent --actor-id <AGENT_ID>
forge-ctl task execute <TASK_ID> --agent-id <AGENT_ID> \
  --role implementer --purpose implement \
  --prompt "Create greeting.txt with a short greeting"
forge-ctl task get <TASK_ID>
```

Replace each placeholder with the exact ID returned by the preceding command.
Execution completion does not itself advance TaskLifecycle or satisfy a Gate.
Read the lifecycle and GateEvaluation, then use the versioned transition with
the exact evaluation ID for any edge that requires Gate admission. The
[CLI reference](cli.md) and [REST API contract](api.md) document those calls.

## Historical migration record: V071–V076

This section records the earlier Main/Project Agent migration contract. It is
kept as migration history and does not describe current Project or Task setup.

The correction is forward-only. Migrations `V059`–`V070` remain unchanged; the
replacement begins at `V071` or later. Legacy conversation/collaboration
messages, IDs, ordering, ordinary bodies, provenance, runtime metadata,
sessions, LCM/memory references, and turn-job history are preserved. Multiple
source threads merge deterministically by timestamp, source ID, and source
sequence. If no single safe Main/Project binding can be inferred, Forge marks
the account or Project `agent_setup_required` instead of guessing or promoting
a Task Worker. Expired/ambiguous leases become finite retry or terminal states,
never silent success. V075 then quarantines the retired Room and membership
tables as `legacy_*`, converts Room-scoped semantic memory to Agent Chat scope,
and rejects any new Room authority record while retaining source provenance.

The Charter, Project artifact, milestone, release, and shared-media metadata
for this change are added by the forward-only
`V076__project_charter_milestones_media.sql` migration. V001–V075 remain
immutable; existing media IDs, URLs, storage keys, metadata, and file bytes are
preserved in place, with no file move/duplication or on-disk layout break.

Projects that predate the Charter model are explicitly
`legacy_unverified`/`charter_setup_required`; migration never fabricates an
approved Charter from old chat, Tasks, memory, or inferred names. The Project
Chat, Tasks, evidence capture, and Document maintenance remain usable. The
Project Agent may draft an adoption Charter from authorized current state, but
only explicit user approval of its exact revision establishes Project truth and
unblocks release. Existing task media IDs, URLs, storage keys, and file bytes
remain in place; migration does not move or duplicate files or claim an on-disk
layout break. If a migration or server restart fails, old media references and
bytes remain usable and physical cleanup is retried separately after checking
attachments and release pins.

## TaskLifecycle and Gate operations

Read and change aggregate Task progress with the exact lifecycle version. A
transition that requires readiness must carry the exact GateEvaluation ID; a
stale evaluation cannot advance the Task. The API and CLI references document
the lifecycle states, idempotency key, reason pair, and Gate admission fields.
