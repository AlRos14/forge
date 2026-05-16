# API Reference

All endpoints are under `/api/v1/`. The MCP endpoint is `POST /mcp`. The default
bind is `127.0.0.1:8080`. **No authentication is required (localhost-only MVP).**
Do not expose Forge to the public internet without an authenticating reverse
proxy.

For the conceptual model behind these endpoints see
[architecture.md](architecture.md).

## REST endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST   | `/api/v1/projects` | Create project |
| GET    | `/api/v1/projects` | List projects |
| GET    | `/api/v1/projects/{id}` | Get project |
| PATCH  | `/api/v1/projects/{id}` | Update project |
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
| POST   | `/api/v1/agents` | Register agent |
| GET    | `/api/v1/agents` | List agents |
| GET    | `/api/v1/agents/{id}` | Get agent |
| GET    | `/api/v1/tasks/{id}/executions` | List executions |
| GET    | `/api/v1/executions/{id}` | Get execution |
| GET    | `/api/v1/executions/{id}/logs` | Get execution logs |
| GET    | `/api/v1/events` | Server-sent events stream |
| POST   | `/mcp` | MCP JSON-RPC endpoint |

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
