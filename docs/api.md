# API Reference

All endpoints are under `/api/v1/`. The MCP endpoint is `POST /mcp`. By default,
Forge binds loopback on an OS-selected port, persists it in `~/.forge/server.json`,
and reuses it on later starts.

Authentication is required on all non-exempt routes. Requests must carry a
`Bearer` token — either a session JWT obtained via `POST /api/v1/auth/login`
or a personal access token (PAT) prefixed `fg_` issued at
`POST /api/v1/auth/tokens`. MCP clients can additionally use an OAuth 2.1
access token (see `/.well-known/oauth-authorization-server`). The
`register`, `login`, `refresh`, and `logout` routes are the only exempt ones.
Do not expose Forge to the public internet without an authenticating reverse
proxy in front of it.

For the conceptual model behind these endpoints see
[architecture.md](architecture.md).

## REST endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST   | `/api/v1/projects` | Create project |
| GET    | `/api/v1/projects` | List projects |
| GET    | `/api/v1/projects/{id}` | Get project |
| PATCH  | `/api/v1/projects/{id}` | Update project |
| GET    | `/api/v1/projects/{id}/project_hook_runs` | List project hook run history |
| POST   | `/api/v1/projects/{id}/repos` | Create repo |
| GET    | `/api/v1/projects/{id}/repos` | List repos |
| POST   | `/api/v1/projects/{id}/tasks` | Create task |
| GET    | `/api/v1/projects/{id}/tasks` | List tasks (paginated, filterable) |
| GET    | `/api/v1/tasks/{id}` | Get task |
| PATCH  | `/api/v1/tasks/{id}` | Update task |
| DELETE | `/api/v1/tasks/{id}` | Soft-delete task |
| POST   | `/api/v1/tasks/{id}/claim` | Claim task (auto-dispatches the executor) |
| POST   | `/api/v1/tasks/{id}/cancel` | Cancel task (idempotent) |
| POST   | `/api/v1/tasks/{id}/archive` | Archive task (hidden from default lists) |
| POST   | `/api/v1/tasks/{id}/transition` | Transition status; entering `review` returns `{task, review}` inline |
| POST   | `/api/v1/tasks/{id}/review` | Re-run the CI steps without changing state |
| GET    | `/api/v1/tasks/{id}/transitions` | Audit log of state transitions |
| POST   | `/api/v1/tasks/{id}/comments` | Create task comment |
| GET    | `/api/v1/tasks/{id}/comments` | List task comments (paginated) |
| DELETE | `/api/v1/comments/{id}` | Delete user-authored comment |
| POST   | `/api/v1/tasks/{id}/media` | Upload task media attachment |
| GET    | `/api/v1/tasks/{id}/media` | List task media attachments (paginated) |
| GET    | `/api/v1/media/{media_id}` | Stream task media bytes |
| DELETE | `/api/v1/media/{media_id}` | Delete task media attachment |
| POST   | `/api/v1/agents` | Register agent |
| GET    | `/api/v1/agents` | List agents |
| GET    | `/api/v1/agents/{id}` | Get agent |
| GET    | `/api/v1/tasks/{id}/executions` | List executions |
| GET    | `/api/v1/executions/{id}` | Get execution |
| GET    | `/api/v1/executions/{id}/logs` | Get execution logs |
| GET    | `/api/v1/events` | Server-sent events stream |
| POST   | `/mcp` | MCP JSON-RPC endpoint |

## Projects

`ProjectResponse` includes `project_hooks`, an array of project-wide hook
rules stored separately from workflow settings. Projects with no configured
rules return an empty array.

`PATCH /api/v1/projects/{id}` accepts the existing `name`, `settings`,
`default_review_config`, `primary_repo_id`, and `paused` fields, plus an
optional `project_hooks` array. When provided, the server validates and stores
the rules in `project.project_hooks_json`; saving rules does not run hook
actions. Omitting `project_hooks` leaves existing rules unchanged; sending an
empty array clears all rules.

Project hook validation rejects unsupported trigger and action types, the
`task.stuck` trigger in v1, empty rule `id`, empty rule `name`, and empty
required action strings such as `dispatch_agent.agent_id`.

### Project Hooks

Project hooks are project-wide automation rules stored on
`ProjectResponse.project_hooks` and updated by `PATCH /api/v1/projects/{id}`.
The v1 evaluator supports `project.all_work_completed`, which fires when the
project has visible non-automation tasks and all of them are in terminal
workflow states. `dispatch_agent` launches a
configured agent, `create_task` creates a task, `add_comment` adds a task
comment, and `notify` creates a notification. `task.stuck` is
deferred to a future stuck-signal change. Run history is available at
`GET /api/v1/projects/{id}/project_hook_runs` with `items` and `next_cursor`
pagination.

## Pagination

All list endpoints use opaque keyset cursors: base64-encoded JSON
`{sort_by, sort_order, last_value, last_id}`. The `db` layer queries `limit + 1`
rows to determine `has_more`. The response field is `items` (not `data`).

### Query parameters

| Param | Description |
|-------|-------------|
| `cursor` | Opaque pagination cursor returned from the previous page |
| `limit` | Page size (default 20, max 100) |
| `sort_by` | `created_at`, `updated_at`, `priority`, `board_position`, `title`, `status`, `agent`, `task_type`, `id` |
| `sort_order` | `asc`, `desc` |
| `status` | Comma-separated status filter |
| `agent_id` | Comma-separated agent filter |
| `assignee_type` | Comma-separated assignee type filter (`agent`, `user`) |
| `assignee_id` | Comma-separated assignee id / user-handle filter |
| `include_cancelled` | Include cancelled tasks (default false unless `status` includes `cancelled`) |
| `include_archived` | Include archived tasks (default false) |
| `include_total` | Include total count in response |

## Task media (rich comment attachments)

Task media stores images, videos, and downloadable files that task comments can
reference from plain Markdown. Media URLs are stable Forge API paths of the form
`/api/v1/media/{media_id}`. They do not expire and remain valid across server
restarts while the media row and stored file still exist.

### Endpoints

| Method | Path | Request | Success |
|--------|------|---------|---------|
| POST | `/api/v1/tasks/{task_id}/media` | `multipart/form-data` with `file` (binary, required) and `author_name` (text, optional) | `201` with `TaskMediaResponse` |
| GET | `/api/v1/tasks/{task_id}/media` | Query params: `cursor`, `limit` (1-100, default 50), `include_total` | `200` with `PaginatedResponse<TaskMediaResponse>` |
| GET | `/api/v1/media/{media_id}` | None | `200` streaming the stored bytes with the recorded `Content-Type` |
| DELETE | `/api/v1/media/{media_id}` | None | `204` with an empty body |

Upload validation failures return `400`; missing tasks, media, or inaccessible
owned projects return `404`; insufficient delete permissions return `403`.
The list response uses the standard pagination envelope with `items`,
`next_cursor`, `has_more`, and `total_count`.

Image and video media are served inline. Other supported content types, plus
any legacy SVG rows, are served with `Content-Disposition` set to
`attachment; filename=...` using a safe filename derived from the stored display
filename.

For owned projects, callers must be project members to upload, list, or stream
task media. Deleting media requires the project `owner` or `admin` role. Legacy
system projects without an owner remain visible to authenticated callers,
matching the project API.

### `TaskMediaResponse`

```json
{
  "id": "media_...",
  "task_id": "task_...",
  "filename": "evidence.png",
  "content_type": "image/png",
  "byte_size": 12345,
  "url": "/api/v1/media/media_...",
  "author_type": "user",
  "author_id": "user_...",
  "author_name": "User",
  "created_at": "2026-05-19T12:00:00Z"
}
```

| Field | Description |
|-------|-------------|
| `id` | Media id |
| `task_id` | Owning task id |
| `filename` | Normalized display filename |
| `content_type` | Recorded MIME type |
| `byte_size` | Stored byte count |
| `url` | Stable Forge API URL: `/api/v1/media/{media_id}` |
| `author_type` | `user`, `agent`, or `system` |
| `author_id` | Optional author id |
| `author_name` | Display name recorded at upload time |
| `created_at` | RFC3339 creation timestamp |

### Safety controls

Supported content types are `image/png`, `image/jpeg`, `image/gif`,
`image/webp`, `video/mp4`, `video/webm`, `video/quicktime`, `application/pdf`,
`text/plain`, and `application/zip`. SVG uploads are rejected because inline
SVG can execute script in the Forge origin.

Blocked filename extensions are `.exe`, `.bat`, `.sh`, `.command`, and `.app`;
they are rejected regardless of the claimed `content_type`.

The per-file upload limit is configured by `server.media_upload_limit_bytes`
(`FORGE_MEDIA_UPLOAD_LIMIT_BYTES` in the environment). The default is 100 MiB
(`104857600` bytes). Uploads above the effective limit return `400`.
Multipart text fields are read with small explicit caps; `author_name` must be
at most 256 bytes.

Filenames are normalized before storage: path separators and control characters
are stripped, surrounding whitespace is trimmed, and names longer than 255 bytes
are rejected. Empty names, `.`, and `..` are also rejected.

Stored files use collision-safe storage keys:
`<task_id>/<uuid>__<safe_filename>`.

### Lifecycle

Task media is stored under `<data_dir>/media/<task_id>/...`, not inside the
task worktree. Workspace cleanup for done tasks does not touch task media, so
media links remain valid for archived, done, and cancelled tasks.

Deleting an individual media item soft-deletes the row by setting `deleted_at`,
removes the stored bytes, and returns `204`. Soft-deleting a task tombstones its
active media rows and removes their stored files. A future hard task delete
cascades remaining media rows through the database foreign key.

### SSE events

`GET /api/v1/events` subscribers receive media lifecycle events through the
existing SSE stream. Event context fields are flattened onto the standard
`ForgeEvent` envelope with `event_type`, `entity_id`, and `timestamp`.

| Event | Context payload |
|-------|-----------------|
| `task.media.uploaded` | `{ "task_id": "...", "media_id": "...", "content_type": "...", "byte_size": 12345, "filename": "evidence.png" }` |
| `task.media.deleted` | `{ "task_id": "...", "media_id": "..." }` |

### Markdown evidence patterns

Comments remain plain Markdown created through
`POST /api/v1/tasks/{id}/comments`. Authors reference uploaded media by using
the `url` returned from `TaskMediaResponse`:

| Media | Markdown |
|-------|----------|
| Image | `![alt](/api/v1/media/{media_id})` |
| Video | `<video src='/api/v1/media/{media_id}' controls></video>` |
| Download | `[filename](/api/v1/media/{media_id})` |

The web UI sanitizes Markdown rendering and only permits image or video `src`
URLs that begin with `/api/v1/media/`.

### CLI evidence helpers

Agents should use REST-backed CLI helpers for proof media:

| Command | Purpose |
|---------|---------|
| `forge-ctl task media upload --task-id <id> --file <path>` | Uploads a file and prints media metadata plus the stable URL |
| `forge-ctl task media comment --task-id <id> --content '<markdown>' --media-url <url>...` | Posts a comment with evidence URLs appended as Markdown references |

MCP media upload is intentionally excluded because binary uploads through MCP
would push bytes into the agent context window.

## Errors

All errors render as:

```json
{
  "code": "version_conflict",
  "message": "task version mismatch",
  "details": { "expected": 3, "actual": 4 },
  "request_id": "req_..."
}
```

Common HTTP mappings:

| Status | When |
|--------|------|
| 400 | Validation failure |
| 404 | Resource not found |
| 409 | Optimistic version conflict, role assignment conflict |
| 412 | Workflow guard rejection (`before_exit` blocked the transition) |
| 422 | Illegal state transition |
| 500 | Internal error |

## Server-Sent Events

`GET /api/v1/events` streams `ForgeEvent` payloads from the in-memory event
bus. Useful for the web UI and for long-running scripts that want to react to
state changes (`task.status_changed`, `execution.completed`, …) without
polling.

## MCP tools

Forge exposes 7 tools at `POST /mcp` (JSON-RPC 2.0). The MCP server has its own
`McpState` and does not depend on the `api` crate.

MCP requests require authentication. Clients can send `Authorization: Bearer
<token>` or include `token=<token>` in the MCP URL query string; `forge-ctl mcp
install` writes the query-string form because the supported client config files
store only the server URL.

| Tool | Purpose |
|------|---------|
| `forge_create_task` | Create a new task |
| `forge_list_tasks` | List tasks with pagination |
| `forge_get_task` | Get task detail |
| `forge_assign_agent` | Atomic claim |
| `forge_cancel_task` | Cancel task |
| `forge_get_task_diff` | Get code diff |
| `forge_list_executions` | List executions |

Disable the endpoint with `forge --no-mcp` if you don't want it.

## Execution logs

Execution chat history is backed by Forge JSONL logs plus execution prompt
metadata, not by agent-private transcript storage. See
[execution-logs.md](execution-logs.md) for the adapter-specific details and
log schema.
