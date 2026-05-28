# Execution Logs and Chat History

Forge stores execution history in Forge-owned JSONL files plus execution turn
metadata. The UI does not read an agent's private transcript database directly.
Each adapter translates the agent or process output into the shared `LogEntry`
envelope:

```json
{
  "schema_version": 1,
  "sequence": 0,
  "timestamp": "2026-04-26T00:00:00Z",
  "execution_id": "execution-id",
  "kind": "assistant",
  "stream": "main",
  "payload": {},
  "truncated": false
}
```

The API serves these entries from `GET /api/v1/executions/{id}/logs`. The web UI
then maps `kind` and adapter-specific `payload` shapes into chat entries.

The prompt sent to the executor is stored separately on `execution.prompt`.
The chat UI renders that prompt as the first user message and deduplicates any
matching user-message event that also appears in the agent stream. This mirrors
Vibe Kanban's approach: product-level execution metadata is the stable source for
turn prompts, while logs remain the source for agent output and tool activity.

## Adapter Sources

Adapters differ in where user-visible chat messages come from.

| Adapter     | Log source                                                    | User prompt handling                                                                                                                                                                             |
| ----------- | ------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Codex       | Codex app-server protocol events normalized into Forge JSONL. | The UI renders `execution.prompt` first. Native Codex `userMessage` events are still parsed for later user turns, but a native event matching `execution.prompt` is skipped to avoid duplicates. |
| Claude Code | Claude Code stream JSON normalized into Forge JSONL.          | The UI renders `execution.prompt` first. Forge may also preserve a `kind: "user"` prompt log for compatibility, but the matching log is skipped in chat to avoid duplicates.                     |
| Cursor      | Cursor Agent stream JSON normalized into Forge JSONL.         | Cursor emits a native `user` event in print mode; the UI renders `execution.prompt` first and skips matching native prompt events to avoid duplicates.                                           |
| Shell       | Process stdout/stderr written into Forge JSONL.               | Shell has no structured agent conversation; stdout/stderr render as shell output.                                                                                                                |

## Resume Identifiers

`execution.agent_session_id` stores the adapter session/thread id used for
follow-ups.

| Adapter     | Resume config key   | Meaning                                                                    |
| ----------- | ------------------- | -------------------------------------------------------------------------- |
| Codex       | `resume_thread_id`  | Existing Codex thread id. Follow-ups fork the thread and start a new turn. |
| Claude Code | `resume_session_id` | Existing Claude session id passed to Claude Code resume mode.              |
| Cursor      | `resume_session_id` | Existing Cursor Agent session id passed to `cursor-agent --resume`.        |

## Follow-Up Turns

Follow-ups are modeled as new executions, not as mutations of the previous
execution's chat history. This matches the Vibe Kanban model:

1. The user submits a follow-up message.
2. Forge creates a new execution with:
   - `parent_execution_id` pointing at the previous execution.
   - `prompt` set to the follow-up message.
   - `summary` initially set to the follow-up message for list display.
3. Forge resolves the parent execution's `agent_session_id`.
4. Forge injects the adapter-specific resume key into the new execution's
   executor config snapshot.
5. The adapter resumes or forks the native session and sends the follow-up
   prompt as the next turn.
6. The UI renders the follow-up prompt from `execution.prompt` and deduplicates
   any matching native user-message event in the JSONL stream.

Adapter behavior:

| Adapter     | Follow-up behavior                                                                                          |
| ----------- | ----------------------------------------------------------------------------------------------------------- |
| Codex       | Uses `resume_thread_id`, forks the existing thread, then starts a new turn with the follow-up prompt.        |
| Claude Code | Uses `resume_session_id`, starts Claude Code in resume mode, then sends the follow-up prompt over stdin JSON. |
| Cursor      | Uses `resume_session_id`, starts `cursor-agent --resume`, then sends the follow-up prompt in print mode.     |

The important distinction is that native agent history is used for agent
context, while Forge execution metadata is used for stable product chat display.

## Test Checklist

Use this checklist when revisiting prompt/resume behavior:

- Initial execution shows exactly one user prompt in chat.
- Completed execution still shows its original prompt even after `summary` is
  replaced with the agent result.
- Follow-up execution shows the follow-up message as the first user prompt.
- Codex follow-up resumes from the parent thread and does not duplicate the
  native `userMessage` event.
- Claude Code follow-up resumes from the parent session and does not require the
  Claude stream to echo the prompt.
- Cursor follow-up resumes from the parent session and deduplicates the native
  Cursor `user` event when it matches `execution.prompt`.
- Raw logs remain unchanged; prompt synthesis affects only chat rendering.

## Important Invariants

- Forge JSONL is the canonical UI/API log format.
- `execution.prompt` is the canonical source for the visible user prompt at the
  start of an execution turn.
- Adapter-native payloads may be preserved inside `payload`, but consumers should
  key off the Forge `kind` first.
- Native user-message events are supplemental. The chat mapper should deduplicate
  events whose text matches `execution.prompt`.
- Older executions may have logs under temporary paths or task workspace
  metadata directories. New execution logs are stored under the durable
  workspace log root at `.forge/logs/<project_id>/<task_id>/`.
