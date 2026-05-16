use serde_json::{json, Value};

pub(crate) fn tool_descriptors(scoped_project: bool) -> Value {
    json!([
        tool_descriptor(
            "forge_create_task",
            "Create a task in a project repository.",
            json!({
                "project_id": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "parent_task_id": { "type": "string" },
                "type": { "type": "string", "enum": ["task", "planning_task", "sub_task"] },
                "priority": { "type": "integer" }
            }),
            required(scoped_project, &["project_id", "title"], &["title"]),
        ),
        tool_descriptor(
            "forge_list_tasks",
            "List tasks for a project.",
            json!({
                "project_id": { "type": "string" },
                "cursor": { "type": "string" },
                "limit": { "type": "integer" },
                "status": {
                    "oneOf": [
                        { "type": "string" },
                        { "type": "array", "items": { "type": "string" } }
                    ]
                },
                "sort_by": { "type": "string", "enum": ["created_at", "updated_at", "priority", "id"] }
            }),
            required(scoped_project, &["project_id"], &[]),
        ),
        tool_descriptor(
            "forge_get_task",
            "Get a task by id.",
            json!({
                "task_id": { "type": "string" }
            }),
            &["task_id"],
        ),
        tool_descriptor(
            "forge_assign_agent",
            "Assign an agent to a task.",
            json!({
                "task_id": { "type": "string" },
                "agent_id": { "type": "string" }
            }),
            &["task_id", "agent_id"],
        ),
        tool_descriptor(
            "forge_cancel_task",
            "Cancel a task.",
            json!({
                "task_id": { "type": "string" }
            }),
            &["task_id"],
        ),
        tool_descriptor(
            "forge_get_task_diff",
            "Get the latest task diff when available.",
            json!({
                "task_id": { "type": "string" }
            }),
            &["task_id"],
        ),
        tool_descriptor(
            "forge_list_executions",
            "List executions for a task.",
            json!({
                "task_id": { "type": "string" },
                "cursor": { "type": "string" },
                "limit": { "type": "integer" }
            }),
            &["task_id"],
        ),
        tool_descriptor(
            "forge_update_task",
            "Update mutable task fields.",
            json!({
                "task_id": { "type": "string" },
                "title": { "type": "string" },
                "description": { "type": "string" },
                "priority": { "type": "integer" },
                "plan": { "type": "string" },
                "version": { "type": "integer" }
            }),
            &["task_id", "version"],
        ),
        tool_descriptor(
            "forge_transition_task",
            "Transition a task to another status.",
            json!({
                "task_id": { "type": "string" },
                "status": { "type": "string", "enum": ["todo", "in_progress", "review", "merging", "merge_failed", "done", "cancelled", "blocked"] },
                "version": { "type": "integer" }
            }),
            &["task_id", "status", "version"],
        ),
        tool_descriptor(
            "forge_register_agent",
            "Register an agent executor.",
            json!({
                "name": { "type": "string" },
                "executor_type": { "type": "string", "enum": ["shell", "codex", "claude_code", "opencode"] },
                "daemon_id": { "type": "string" }
            }),
            &["name", "executor_type"],
        ),
        tool_descriptor(
            "forge_list_agents",
            "List registered agents.",
            json!({
                "status": { "type": "string", "enum": ["idle", "busy", "error", "offline"] },
                "cursor": { "type": "string" },
                "limit": { "type": "integer" }
            }),
            &[],
        ),
        tool_descriptor(
            "forge_list_projects",
            "List projects.",
            json!({
                "cursor": { "type": "string" },
                "limit": { "type": "integer" }
            }),
            &[],
        ),
        tool_descriptor(
            "forge_create_project",
            "Create a project.",
            json!({
                "name": { "type": "string" }
            }),
            &["name"],
        ),
        tool_descriptor(
            "forge_get_project",
            "Get a project by id, including settings and lifecycle hooks.",
            json!({
                "project_id": { "type": "string" }
            }),
            required(scoped_project, &["project_id"], &[]),
        ),
        tool_descriptor(
            "forge_update_project",
            "Update mutable project fields. Settings are replaced when provided and must pass project settings validation.",
            json!({
                "project_id": { "type": "string" },
                "name": { "type": "string" },
                "settings": { "type": "object" },
                "paused": { "type": "boolean" }
            }),
            required(scoped_project, &["project_id"], &[]),
        ),
        tool_descriptor(
            "forge_update_project_lifecycle_hooks",
            "Replace a project's lifecycle hooks without changing other project settings.",
            json!({
                "project_id": { "type": "string" },
                "lifecycle_hooks": {
                    "type": "object",
                    "description": "Map of lifecycle event names to hook arrays. Events: before_work, on_work_start, on_work_stop, on_task_done, on_task_cancel.",
                    "additionalProperties": {
                        "type": "array",
                        "items": {
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "type": "string", "enum": ["script"] },
                                        "command": { "type": "string" },
                                        "timeout_seconds": { "type": "integer", "minimum": 1 },
                                        "blocking": { "type": "boolean" }
                                    },
                                    "required": ["type", "command"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "type": "string", "enum": ["plugin"] },
                                        "name": { "type": "string" },
                                        "enabled": { "type": "boolean" },
                                        "config": { "type": "object" }
                                    },
                                    "required": ["type", "name"]
                                }
                            ]
                        }
                    }
                }
            }),
            required(
                scoped_project,
                &["project_id", "lifecycle_hooks"],
                &["lifecycle_hooks"],
            ),
        ),
        tool_descriptor(
            "forge_follow_up_execution",
            "Send a follow-up message to resume an agent session on a completed or failed execution. Creates a child execution that carries forward conversation context.",
            json!({
                "execution_id": { "type": "string", "description": "ID of the parent execution to follow up on" },
                "message": { "type": "string", "description": "The follow-up instruction for the agent" },
                "agent_id": { "type": "string", "description": "Optional: override agent (must be same executor type)" },
                "overrides": {
                    "type": "object",
                    "properties": {
                        "model_id": { "type": "string" },
                        "reasoning_effort": { "type": "string" },
                        "permission_policy": { "type": "string" }
                    }
                }
            }),
            &["execution_id", "message"],
        ),
        tool_descriptor(
            "forge_add_task_dependency",
            "Declare that a task depends on another task. The dependent task cannot be freely claimed until the prerequisite task reaches 'done'. Cycles are rejected.",
            json!({
                "task_id": { "type": "string", "description": "The task that has the dependency (blocked task)" },
                "depends_on_id": { "type": "string", "description": "The prerequisite task that must reach 'done' first" }
            }),
            &["task_id", "depends_on_id"],
        ),
        tool_descriptor(
            "forge_remove_task_dependency",
            "Remove a dependency between two tasks.",
            json!({
                "task_id": { "type": "string", "description": "The dependent task" },
                "depends_on_id": { "type": "string", "description": "The prerequisite task to remove" }
            }),
            &["task_id", "depends_on_id"],
        ),
        tool_descriptor(
            "forge_list_task_dependencies",
            "List the dependencies of a task — the prerequisite tasks that must complete before this task can be freely claimed.",
            json!({
                "task_id": { "type": "string" }
            }),
            &["task_id"],
        ),
        tool_descriptor(
            "forge_create_sub_tasks",
            "Create subtasks under a root task. Order in the array determines display order.",
            json!({
                "parent_task_id": { "type": "string" },
                "subtasks": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": { "type": "string" },
                            "description": { "type": "string" },
                            "assignee_id": { "type": "string" }
                        },
                        "required": ["title"]
                    }
                }
            }),
            &["parent_task_id", "subtasks"],
        ),
    ])
}

fn required<'a>(
    scoped_project: bool,
    global: &'a [&'a str],
    scoped: &'a [&'a str],
) -> &'a [&'a str] {
    if scoped_project {
        scoped
    } else {
        global
    }
}

fn tool_descriptor(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
        }
    })
}
