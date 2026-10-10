use serde_json::{json, Value};

pub(crate) fn tool_descriptors(scoped_project: bool) -> Value {
    json!([
        tool_descriptor("forge_create_project", "Create an ordinary project.", json!({"name":{"type":"string"}}), &["name"]),
        tool_descriptor("forge_list_projects", "List projects.", json!({"cursor":{"type":"string"},"limit":{"type":"integer"}}), &[]),
        tool_descriptor("forge_get_project", "Get a project and its generic lifecycle hooks.", json!({"project_id":{"type":"string"}}), &required(scoped_project,&["project_id"],&[])),
        tool_descriptor("forge_update_project", "Update ordinary project identity or pause state.", json!({"project_id":{"type":"string"},"name":{"type":"string"},"paused":{"type":"boolean"}}), &required(scoped_project,&["project_id"],&[])),
        tool_descriptor("forge_update_project_hooks", "Replace generic Project hook rules using the expected Project version.", json!({"project_id":{"type":"string"},"version":{"type":"integer"},"project_hooks":{"type":"array","items":{"type":"object"}}}), &required(scoped_project,&["project_id","version","project_hooks"],&["version","project_hooks"])),
        tool_descriptor("forge_create_task", "Create an ordinary task.", json!({"project_id":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"parent_task_id":{"type":"string"},"type":{"type":"string","enum":["implementation","planning","discovery","review","validation"]},"priority":{"type":"integer"}}), &required(scoped_project,&["project_id","title"],&["title"])),
        tool_descriptor("forge_create_sub_tasks", "Create ordinary subtasks under a task.", json!({"parent_task_id":{"type":"string"},"subtasks":{"type":"array","items":{"type":"object","properties":{"title":{"type":"string"},"description":{"type":"string"}},"required":["title"]}}}), &["parent_task_id","subtasks"]),
        tool_descriptor("forge_list_tasks", "List tasks using TaskLifecycle state.", json!({"project_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"},"lifecycle_state":{"oneOf":[{"type":"string"},{"type":"array","items":{"type":"string","enum":["backlog","ready","active","blocked","ready_to_merge","merging","done","cancelled"]}}]},"sort_by":{"type":"string","enum":["created_at","updated_at","priority","board_position","lifecycle_state","id"]}}), &required(scoped_project,&["project_id"],&[])),
        tool_descriptor("forge_get_task", "Get a task with its authoritative TaskLifecycle and TaskRoles.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_update_task", "Update task description, title, or priority.", json!({"task_id":{"type":"string"},"title":{"type":"string"},"description":{"type":"string"},"priority":{"type":"integer"},"version":{"type":"integer"}}), &["task_id","version"]),
        tool_descriptor("forge_get_task_diff", "Get the current task workspace diff.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_list_executions", "List exact historical Executions for a task.", json!({"task_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &["task_id"]),
        tool_descriptor("forge_start_execution", "Start an Agent Execution with an explicit Task, active TaskRole, Agent, purpose, prompt, and exact Artifact inputs.", json!({"task_id":{"type":"string"},"agent_id":{"type":"string"},"role":{"type":"string"},"purpose":{"type":"string","enum":["plan","implement","review","validate","investigate","orchestrate","general"]},"prompt":{"type":"string"},"input_artifact_ids":{"type":"array","items":{"type":"string"}}}), & ["task_id","agent_id","role","purpose","prompt"]),
        tool_descriptor("forge_follow_up_execution", "Create an explicit child Execution from the specified parent Execution and Agent identity.", json!({"execution_id":{"type":"string"},"message":{"type":"string"},"agent_id":{"type":"string"},"overrides":{"type":"object"}}), &["execution_id","message","agent_id"]),
        tool_descriptor("forge_add_task_dependency", "Add a task dependency.", json!({"task_id":{"type":"string"},"depends_on_id":{"type":"string"}}), &["task_id","depends_on_id"]),
        tool_descriptor("forge_remove_task_dependency", "Remove a task dependency.", json!({"task_id":{"type":"string"},"depends_on_id":{"type":"string"}}), &["task_id","depends_on_id"]),
        tool_descriptor("forge_list_task_dependencies", "List exact task dependencies.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_register_agent", "Register an Agent harness identity.", json!({"name":{"type":"string"},"executor_type":{"type":"string"},"daemon_id":{"type":"string"}}), &["name","executor_type"]),
        tool_descriptor("forge_list_agents", "List Agent harness identities.", json!({"status":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &[]),
        tool_descriptor("forge_list_agent_profiles", "List immutable Harness profiles for an Agent. Credentials are never returned.", json!({"identity_id":{"type":"string"}}), &["identity_id"]),
        tool_descriptor("forge_get_task_lifecycle", "Get the authoritative TaskLifecycle record.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_list_task_lifecycle_transitions", "List exact durable TaskLifecycle transition receipts.", json!({"task_id":{"type":"string"}}), &["task_id"]),
    tool_descriptor("forge_transition_task_lifecycle", "Request a deterministic TaskLifecycle transition against its exact version. Merge-readiness edges require the exact GateEvaluation. reason_kind and reason_ref must be provided together, or both omitted.", json!({"task_id":{"type":"string"},"to_state":{"type":"string","enum":["backlog","ready","active","blocked","ready_to_merge","merging","done","cancelled"]},"expected_lifecycle_version":{"type":"integer"},"idempotency_key":{"type":"string"},"gate_evaluation_id":{"type":"string"},"reason_kind":{"type":"string"},"reason_ref":{"type":"string"}}), &["task_id","to_state","expected_lifecycle_version","idempotency_key"]),
        tool_descriptor("forge_list_task_roles", "List TaskRole and RoleMembership records.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_create_task_role", "Create a Task-scoped Role with an explicit coordination mode and policy.", json!({"task_id":{"type":"string"},"role":{"type":"string"},"coordination_mode":{"type":"string","enum":["partitioned","collaborative","independent"]},"policy":{"type":"object"}}), &["task_id","role","coordination_mode"]),
        tool_descriptor("forge_add_task_role_member", "Add an exact Human or Agent ActorRef to a TaskRole.", json!({"task_id":{"type":"string"},"role":{"type":"string"},"actor_ref":{"type":"object","properties":{"kind":{"type":"string","enum":["human","agent"]},"id":{"type":"string"}},"required":["kind","id"],"additionalProperties":false}}), &["task_id","role","actor_ref"]),
        tool_descriptor("forge_create_task_gate", "Create a deterministic Gate with an explicit policy.", json!({"task_id":{"type":"string"},"gate_kind":{"type":"string"},"policy":{"type":"object"}}), &["task_id","gate_kind","policy"]),
        tool_descriptor("forge_get_gate", "Get one exact Gate and active policy revision.", json!({"gate_id":{"type":"string"}}), &["gate_id"]),
        tool_descriptor("forge_revise_gate_policy", "Create a Gate policy revision using the expected active revision.", json!({"gate_id":{"type":"string"},"expected_active_revision":{"type":"integer"},"policy":{"type":"object"}}), &["gate_id","policy"]),
        tool_descriptor("forge_evaluate_gate", "Evaluate one exact Gate against its current authoritative inputs.", json!({"gate_id":{"type":"string"}}), &["gate_id"]),
        tool_descriptor("forge_get_gate_evaluation", "Get one exact GateEvaluation and its frozen input facts.", json!({"evaluation_id":{"type":"string"}}), &["evaluation_id"]),
        tool_descriptor("forge_list_task_review_executions", "List exact reviewer+review Executions and their ReviewReport outputs.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_get_review_execution", "Get one exact reviewer+review Execution and its ReviewReport output.", json!({"execution_id":{"type":"string"}}), &["execution_id"]),
        tool_descriptor("forge_list_validation_runs", "List exact ValidationRun records for a task.", json!({"task_id":{"type":"string"}}), &["task_id"]),
        tool_descriptor("forge_get_validation_run", "Get one exact ValidationRun record.", json!({"validation_run_id":{"type":"string"}}), &["validation_run_id"]),
        tool_descriptor("forge_get_evidence", "Get one exact Evidence record and its producer identity.", json!({"evidence_id":{"type":"string"}}), &["evidence_id"]),
        tool_descriptor("forge_list_task_artifacts", "List exact Artifact records attached to a task.", json!({"task_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &["task_id"]),
        tool_descriptor("forge_list_task_messages", "List generic task collaboration Messages.", json!({"task_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &["task_id"]),
        tool_descriptor("forge_list_task_handoffs", "List generic task collaboration Handoffs.", json!({"task_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &["task_id"]),
        tool_descriptor("forge_list_task_proposals", "List generic task Proposals.", json!({"task_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &["task_id"]),
        tool_descriptor("forge_list_task_decisions", "List generic task Decisions with exact proposal provenance.", json!({"task_id":{"type":"string"},"cursor":{"type":"string"},"limit":{"type":"integer"}}), &["task_id"])
    ])
}

fn required<'a>(
    scoped_project: bool,
    always: &'a [&'a str],
    without_scope: &'a [&'a str],
) -> Vec<&'a str> {
    let fields = if scoped_project {
        without_scope
    } else {
        always
    };
    fields.to_vec()
}

fn tool_descriptor(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
    })
}
