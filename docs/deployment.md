# Deployment and container permissions

This page covers the supported local container deployment for this repository.
Each release publishes a multi-tag image to `ghcr.io/alros14/forge`
(`<version>`, `<major>.<minor>`, and `latest`). You can also build the image
from the checked-out source.

## Identity and filesystem contract

The image creates group and user `forge` with UID/GID `10001`. The entrypoint
starts as root only to inspect `/data`, performs no ownership change unless
explicitly authorized, and then `exec`s the server as `forge`. The Forge process
and its embedded shell, Git, and workspace operations never run as root.

The server needs write access to:

- `/data` for SQLite, the JWT secret, `server.json`, logs, sessions, and media;
- the configured workspace root (`/tmp/forge/worktrees` by default);
- any mounted repository/worktree path it is asked to use.

Do not solve permission failures with `chmod 777` or a blind recursive
`chown`. Fix the specific mount or use the documented migration below.

## Clean install

From the published image, without a source checkout, using the same settings as
the repository `docker-compose.yml`:

```bash
docker run -d --name forge --init --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v forge-data:/data \
  ghcr.io/alros14/forge:0.9.2
docker logs -f forge
```

From the checked-out source with Compose:

```bash
docker compose build
docker compose up -d
docker compose logs -f forge
```

Set `FORGE_PORT` to publish a different host port if `8080` is taken.

Compose binds the host port to `127.0.0.1:8080` by default. Open the URL printed
by Forge, or `http://127.0.0.1:8080`. The named volume `forge-data` persists
`/data` across container replacement.

Verify the runtime identity and persistence:

```bash
docker compose top forge
docker compose exec --user 10001:10001 forge id
docker compose restart forge
docker compose logs forge
```

With `init: true`, PID 1 is `docker-init` and runs as root; the actual `forge`
server process must report UID/GID `10001`. After restart, Projects, accounts,
and server state must remain. The database is `/data/forge.db`; migrations run
on startup and are forward-only.

## Existing root-owned `/data` volumes

Earlier images ran Forge as root. A non-empty root-owned volume fails closed with
an explicit message; no files are changed automatically. If the volume contains
only this Forge installation's data, authorize the one-time migration:

```bash
FORGE_MIGRATE_ROOT_OWNED_DATA=1 docker compose up -d
```

The entrypoint rewrites ownership only under `/data`, only for root-owned
entries, and logs the action. Start once without the variable afterward:

```bash
docker compose up -d
```

If the volume also contains files that must remain owned by another account, do
not use the recursive migration. Copy the Forge data into a new volume or fix
the specific paths manually. Workspace roots and repository mounts are never
recursively rewritten; a root-owned workspace entry also fails closed.

A bind-mounted host directory keeps its host ownership. Either run the container
with an explicit matching `--user`/Compose `user:` value or pre-create the data
directory with UID/GID `10001`. When the process is not started as root, the
entrypoint executes the server directly and does not attempt privileged changes.

## Network exposure

`FORGE_BIND_ADDRESS` defaults to `127.0.0.1` in Compose. Do not set it to
`0.0.0.0` on an untrusted network. Publishing Forge beyond loopback requires:

- a TLS-terminating reverse proxy or private network boundary;
- an explicit `FORGE_SERVER_BIND=0.0.0.0:8080` for the container and a reviewed
  host port mapping;
- `FORGE_PUBLIC_BASE_URL` and `FORGE_CORS_ORIGINS` set to the real origin;
- a reviewed authentication and account policy;
- protection of `/data`, including `jwt_secret.bin`, `forge.db`, provider
  credentials, execution logs, and terminal history.

The direct server binary still defaults to `127.0.0.1:0` and selects a loopback
port. The container's `0.0.0.0` bind is an internal-container setting; host
exposure is controlled by the published port.

## Secrets and logs

- Secrets belong in environment variables or mounted secret files, not in image
  layers, Compose files committed to Git, MCP URLs, or query strings.
- CredentialService encrypts provider and GitHub SCM credentials using a key
  derived from the resolved JWT secret. Preserve the same configured secret or
  the data-directory `jwt_secret.bin` with `forge.db` across backup and restore.
  Changing the resolved secret makes stored credential ciphertext
  undecryptable; reconnect those credentials after an intentional rotation.
- Execution logs and terminal transcripts may contain sensitive command output.
  Treat `/data` as private user data.
- Request logging records the path without the query string. Authentication
  failures do not echo credentials.
- `forge-ctl mcp install` never writes a literal Bearer token into a
  project/local MCP configuration. It writes the client's environment-reference
  syntax and fails closed unless the referenced variable is set. `user`-scope
  files may contain a literal token and are written with `0600` permissions.
- Claude Code project config is `<repo>/.mcp.json`; local and user configs share
  `~/.claude.json` (project-keyed `projects[...].mcpServers` and top-level
  `mcpServers` respectively). Existing unrelated keys and MCP entries are
  preserved.
- MCP configuration writes resolve through directory descriptors and reject
  symlinks in intermediate directories and the final file. A symlinked config
  path is a hard failure, not a redirect. `forge-ctl mcp` applies the same rule
  to Claude, Codex, and Cursor files before install, status, or uninstall.
- Terminal input and resize frames recheck the current Project write authority;
  revoking membership or deleting the account rejects the next frame.

## Controlled failure

If `/data` or a workspace is not writable by `forge`, startup fails with a
permission error instead of silently running with reduced persistence. The
container does not fall back to root. Inspect `docker compose logs forge` and fix
the exact mount ownership.
