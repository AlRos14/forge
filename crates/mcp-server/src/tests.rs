use std::{future::Future, sync::Arc};

use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, Agent, AgentRepo, AgentStatus,
    CreateAgent, CreateExecution, CreateProject, CreateProjectMember, CreateRepo,
    CreateRoleMembership, CreateTask, CreateTaskRole, CreateWorkspace, DaemonRepo, DaemonStatus,
    ExecutionPurpose, ExecutionRepo, ExecutionStatus, ProjectMemberRepo, ProjectRepo, RepoRepo,
    RoleMembershipRepo, RoleMembershipStatus, SqliteDb, Task, TaskRepo, TaskRoleRepo,
    UpdateProject, UpdateTask, UpsertDaemon, User, UserRepo, ValidationRunRepo, WorkspaceRepo,
    WorkspaceStatus,
};
use events::EventBus;
use serde_json::{json, Value};

use crate::{
    protocol::McpContext,
    rpc::{dispatch, dispatch_with_context},
    AppState,
};

fn run_async<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime builds")
        .block_on(future)
}

async fn sqlite_state() -> AppState {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    let db = Arc::new(SqliteDb::new(pool));
    let now = now_rfc3339();
    UserRepo::create_user(
        &*db,
        &User {
            id: "test-user".to_owned(),
            email: "test-user@example.com".to_owned(),
            password_hash: "test".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("test user creates");
    let admin_now = now_rfc3339();
    UserRepo::create_user(
        &*db,
        &User {
            id: "test-admin".to_owned(),
            email: "test-admin@example.com".to_owned(),
            password_hash: "test".to_owned(),
            display_name: None,
            is_admin: true,
            created_at: admin_now.clone(),
            updated_at: admin_now,
        },
    )
    .await
    .expect("test admin creates");
    AppState::new(db, Arc::new(EventBus::new(16)))
}

async fn seed_project_repo(state: &AppState) -> (String, String) {
    let now = now_rfc3339();
    let project = ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: new_uuid_v4(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some("test-user".to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    let repo = RepoRepo::create(
        &*state.db,
        CreateRepo {
            id: new_uuid_v4(),
            project_id: project.id.clone(),
            name: "forge".to_owned(),
            local_path: None,
            remote_url: "https://example.com/forge.git".to_owned(),
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("repo creates");
    ProjectRepo::update(
        &*state.db,
        UpdateProject {
            id: project.id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo.id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("project primary repo updates");
    (project.id, repo.id)
}

async fn add_user(state: &AppState, user_id: &str) {
    let now = now_rfc3339();
    UserRepo::create_user(
        &*state.db,
        &User {
            id: user_id.to_owned(),
            email: format!("{user_id}@example.com"),
            password_hash: "test".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("user creates");
}

fn user_mcp_context(user_id: &str) -> McpContext {
    McpContext {
        project_id: None,
        user_id: Some(user_id.to_owned()),
        is_admin: false,
    }
}

async fn seed_private_project_repo(
    state: &AppState,
    owner_id: &str,
    name: &str,
) -> (String, String) {
    let now = now_rfc3339();
    let project = ProjectRepo::create(
        &*state.db,
        CreateProject {
            id: new_uuid_v4(),
            name: name.to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(owner_id.to_owned()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("private project creates");
    let repo = RepoRepo::create(
        &*state.db,
        CreateRepo {
            id: new_uuid_v4(),
            project_id: project.id.clone(),
            name: "repo".to_owned(),
            local_path: None,
            remote_url: "https://example.com/private.git".to_owned(),
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("private project repo creates");
    ProjectRepo::update(
        &*state.db,
        UpdateProject {
            id: project.id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo.id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("private project repo becomes primary");
    (project.id, repo.id)
}

async fn seed_private_agent(state: &AppState, owner_id: &str) -> String {
    let now = now_rfc3339();
    let agent = AgentRepo::create(
        &*state.db,
        CreateAgent {
            id: new_uuid_v4(),
            name: "private harness".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: r#"{"api_key":"private-value","nested":{"password":"nested-value"},"safe":"visible"}"#.to_owned(),
            credential_ref: None,
            daemon_id: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(owner_id.to_owned()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("private Agent creates");
    agent.id
}

async fn seed_validation_run_and_evidence(state: &AppState, task: &Task) -> (String, String) {
    let now = now_rfc3339();
    let workspace_id = new_uuid_v4();
    WorkspaceRepo::create(
        &*state.db,
        CreateWorkspace {
            id: workspace_id.clone(),
            task_id: task.id.clone(),
            repo_id: task.repo_id.clone().expect("fixture task has a repo"),
            worktree_path: format!("/tmp/forge-test/{}", task.id),
            branch: format!("test/{}", task.id),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("workspace creates");

    let run_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO validation_run
            (id, task_id, check_identity, command, config_summary_json, config_digest,
             workspace_id, commit_sha, workspace_snapshot_digest, idempotency_key, status,
             started_at, created_at, updated_at)
         VALUES (?, ?, 'ci:test', 'true', '{}', ?, ?, '1234567', ?, ?, 'running', ?, ?, ?)",
    )
    .bind(&run_id)
    .bind(&task.id)
    .bind("a".repeat(64))
    .bind(&workspace_id)
    .bind("b".repeat(64))
    .bind(format!("validation-{run_id}"))
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(state.db.pool())
    .await
    .expect("ValidationRun fixture inserts");

    let evidence_id = new_uuid_v4();
    let evidence_content = json!({
        "validation_run_id": run_id,
        "task_id": task.id,
        "check_identity": "ci:test",
        "command": "true",
        "config_digest": "a".repeat(64),
        "workspace_id": workspace_id,
        "commit_sha": "1234567",
        "workspace_snapshot_digest": "b".repeat(64),
    })
    .to_string();
    let mut transaction = state
        .db
        .pool()
        .begin()
        .await
        .expect("fixture transaction begins");
    sqlx::query(
        "INSERT INTO evidence_validation_run_producer
            (evidence_id, validation_run_id, task_id, evidence_key)
         VALUES (?, ?, ?, 'test:evidence')",
    )
    .bind(&evidence_id)
    .bind(&run_id)
    .bind(&task.id)
    .execute(&mut *transaction)
    .await
    .expect("Evidence producer inserts");
    sqlx::query(
        "INSERT INTO evidence (id, task_id, kind, content_json, digest, created_at)
         VALUES (?, ?, 'deterministic_check_output', ?, ?, ?)",
    )
    .bind(&evidence_id)
    .bind(&task.id)
    .bind(evidence_content)
    .bind("c".repeat(64))
    .bind(&now)
    .execute(&mut *transaction)
    .await
    .expect("Evidence fixture inserts");
    transaction
        .commit()
        .await
        .expect("fixture transaction commits");
    (run_id, evidence_id)
}

async fn seed_task(state: &AppState) -> Task {
    let (project_id, _repo_id) = seed_project_repo(state).await;
    seed_task_in_project(state, project_id).await
}

async fn seed_task_in_project(state: &AppState, project_id: String) -> Task {
    state
        .task_service
        .create_task(
            project_id,
            "Ship MCP",
            Some("initial".to_owned()),
            None,
            Some(1),
            None,
            None,
            None,
        )
        .await
        .expect("task creates")
}

async fn seed_execution(state: &AppState, task_id: String) -> String {
    let now = now_rfc3339();
    let execution_id = new_uuid_v4();
    ExecutionRepo::create(
        &*state.db,
        CreateExecution {
            id: execution_id.clone(),
            task_id,
            agent_id: None,
            actor_ref: None,
            purpose: None,
            harness_session_id: None,
            role: "coder".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("execution creates");
    execution_id
}

async fn seed_task_with_legacy_resume_hint(state: &AppState, ambiguous_history: bool) -> Task {
    let task = seed_task(state).await;
    let agent = seed_agent(state, "Legacy resume agent").await;
    let now = now_rfc3339();
    let execution_id = new_uuid_v4();
    ExecutionRepo::create(
        &*state.db,
        CreateExecution {
            id: execution_id.clone(),
            task_id: task.id.clone(),
            agent_id: Some(agent.id.clone()),
            actor_ref: Some(db::ActorRef::Agent(agent.id.clone())),
            purpose: Some(ExecutionPurpose::Implement),
            harness_session_id: None,
            role: "implementer".to_owned(),
            status: ExecutionStatus::Failed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("legacy execution creates");
    let annotation = json!({
        "type": "executor_failed",
        "blocking_reason": "execution stopped",
        "blocked_execution_id": execution_id,
        "recovery_actions": ["resume_session"]
    })
    .to_string();
    TaskRepo::update(
        &*state.db,
        UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(annotation)),
            blocked_json: None,
            failed_json: None,
            parent_task_id: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("resume hint persists");

    if ambiguous_history {
        sqlx::query(
            "INSERT INTO execution_session_migration_issue
                 (id, execution_id, issue_kind, details_json, created_at)
             VALUES (?, ?, 'historical_session_ambiguous', '{}', ?)",
        )
        .bind(new_uuid_v4())
        .bind(&execution_id)
        .bind(now)
        .execute(state.db.pool())
        .await
        .expect("ambiguous historical evidence records");
    }
    task
}

async fn seed_agent_registration_deps(state: &AppState) -> (String, String) {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();
    let host = DaemonRepo::upsert_by_machine_id(
        &*state.db,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: format!("machine-{daemon_id}"),
            hostname: "test-host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("daemon creates");

    ("shell".to_owned(), host.id)
}

#[allow(dead_code)]
async fn seed_agent(state: &AppState, name: &str) -> Agent {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();
    let daemon = DaemonRepo::upsert_by_machine_id(
        &*state.db,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: format!("machine-{daemon_id}"),
            hostname: "test-host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: None,
            labels_json: "{}".to_owned(),
            status: DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon creates");

    AgentRepo::create(
        &*state.db,
        CreateAgent {
            id: new_uuid_v4(),
            name: name.to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon.id),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            prompt_template: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("agent creates")
}

#[allow(dead_code)]
async fn assign_coder(state: &AppState, task_id: &str, agent_id: &str) {
    let now = now_rfc3339();
    let role = TaskRoleRepo::create(
        &*state.db,
        CreateTaskRole {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            role: "implementer".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("implementer TaskRole creates");
    RoleMembershipRepo::add(
        &*state.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role.id,
            actor_kind: db::ActorKind::Agent,
            actor_id: agent_id.to_owned(),
            status: RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Agent joins implementer TaskRole");
}

async fn call_tool(state: &AppState, name: &str, arguments: Value) -> Value {
    let result = dispatch(
        state,
        "tools/call",
        json!({
            "name": name,
            "arguments": arguments,
        }),
    )
    .await
    .expect("tool succeeds");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("tool result includes text content");
    serde_json::from_str(text).expect("tool text content is JSON")
}

fn admin_mcp_context() -> McpContext {
    McpContext {
        project_id: None,
        user_id: Some("test-admin".to_owned()),
        is_admin: true,
    }
}

async fn call_tool_as(
    state: &AppState,
    context: &McpContext,
    name: &str,
    arguments: Value,
) -> Value {
    let result = dispatch_with_context(
        state,
        context,
        "tools/call",
        json!({
            "name": name,
            "arguments": arguments,
        }),
    )
    .await
    .expect("tool succeeds");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("tool result includes text content");
    serde_json::from_str(text).expect("tool text content is JSON")
}

async fn call_tool_error_as(
    state: &AppState,
    context: &McpContext,
    name: &str,
    arguments: Value,
) -> crate::protocol::McpError {
    let error = dispatch_with_context(
        state,
        context,
        "tools/call",
        json!({
            "name": name,
            "arguments": arguments,
        }),
    )
    .await
    .expect_err("tool call should fail");
    error
        .into_response(json!(1))
        .error
        .expect("error response has error")
}

async fn call_tool_scoped(
    state: &AppState,
    project_id: &str,
    name: &str,
    arguments: Value,
) -> Value {
    let result = dispatch_with_context(
        state,
        &McpContext {
            project_id: Some(project_id.to_owned()),
            user_id: Some("test-user".to_owned()),
            is_admin: false,
        },
        "tools/call",
        json!({
            "name": name,
            "arguments": arguments,
        }),
    )
    .await
    .expect("tool succeeds");
    let text = result["content"][0]["text"]
        .as_str()
        .expect("tool result includes text content");
    serde_json::from_str(text).expect("tool text content is JSON")
}

async fn call_tool_error(
    state: &AppState,
    name: &str,
    arguments: Value,
) -> crate::protocol::McpError {
    let error = dispatch(
        state,
        "tools/call",
        json!({
            "name": name,
            "arguments": arguments,
        }),
    )
    .await
    .expect_err("tool call should fail");
    let response = error.into_response(json!(1));
    response.error.expect("error payload present")
}

#[test]
fn initialize_returns_correct_protocol_version() {
    run_async(async {
        let state = sqlite_state().await;
        let result = dispatch(&state, "initialize", json!({}))
            .await
            .expect("initialize succeeds");
        assert_eq!(result["protocolVersion"], "2025-03-26");
        assert_eq!(result["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    });
}

#[test]
fn tools_list_returns_descriptors() {
    run_async(async {
        let state = sqlite_state().await;
        let result = dispatch(&state, "tools/list", json!({}))
            .await
            .expect("tools/list succeeds");
        let tools = result["tools"]
            .as_array()
            .expect("tools/list returns {tools:[...]}");
        let names = tools
            .iter()
            .map(|tool| tool["name"].as_str().expect("tool name").to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        let expected = [
            "forge_add_task_dependency",
            "forge_add_task_role_member",
            "forge_create_project",
            "forge_create_task_role",
            "forge_create_sub_tasks",
            "forge_create_task",
            "forge_follow_up_execution",
            "forge_get_evidence",
            "forge_get_gate",
            "forge_get_gate_evaluation",
            "forge_get_project",
            "forge_get_review_execution",
            "forge_get_task",
            "forge_get_task_diff",
            "forge_get_task_lifecycle",
            "forge_get_validation_run",
            "forge_list_agent_profiles",
            "forge_list_agents",
            "forge_list_executions",
            "forge_list_projects",
            "forge_list_task_artifacts",
            "forge_list_task_decisions",
            "forge_list_task_dependencies",
            "forge_list_task_handoffs",
            "forge_list_task_lifecycle_transitions",
            "forge_list_task_messages",
            "forge_list_task_proposals",
            "forge_list_task_review_executions",
            "forge_list_task_roles",
            "forge_list_tasks",
            "forge_list_validation_runs",
            "forge_register_agent",
            "forge_remove_task_dependency",
            "forge_revise_gate_policy",
            "forge_create_task_gate",
            "forge_evaluate_gate",
            "forge_transition_task_lifecycle",
            "forge_start_execution",
            "forge_update_project",
            "forge_update_project_hooks",
            "forge_update_task",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(names, expected);
        assert!(tools
            .iter()
            .any(|tool| tool.get("name").is_some() && tool.get("inputSchema").is_some()));
        let follow_up = tools
            .iter()
            .find(|tool| tool["name"] == "forge_follow_up_execution")
            .expect("follow-up tool descriptor exists");
        assert!(follow_up["inputSchema"]["required"]
            .as_array()
            .expect("follow-up required params are an array")
            .iter()
            .any(|required| required == "idempotency_key"));
    });
}

#[test]
fn task_role_membership_tools_require_identity_and_keep_exact_actor_ref() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task(&state).await;
        let agent = seed_agent(&state, "Role member").await;
        let create_args = json!({
            "task_id": task.id,
            "role": "implementer",
            "coordination_mode": "independent"
        });
        let unauthenticated = dispatch_with_context(
            &state,
            &McpContext::default(),
            "tools/call",
            json!({
                "name": "forge_create_task_role",
                "arguments": create_args.clone()
            }),
        )
        .await
        .expect_err("role mutation requires an authenticated user");
        assert_eq!(unauthenticated.code, -32003);

        let context = McpContext {
            project_id: None,
            user_id: Some("test-user".to_owned()),
            is_admin: false,
        };
        let created = dispatch_with_context(
            &state,
            &context,
            "tools/call",
            json!({
                "name": "forge_create_task_role",
                "arguments": create_args
            }),
        )
        .await
        .expect("TaskRole creates");
        let created: Value = serde_json::from_str(
            created["content"][0]["text"]
                .as_str()
                .expect("tool response text"),
        )
        .expect("TaskRole JSON");
        assert_eq!(created["role"], "implementer");
        assert_eq!(created["members"], json!([]));

        let member = dispatch_with_context(
            &state,
            &context,
            "tools/call",
            json!({
                "name": "forge_add_task_role_member",
                "arguments": {
                    "task_id": task.id,
                    "role": "implementer",
                    "actor_ref": {"kind": "agent", "id": agent.id}
                }
            }),
        )
        .await
        .expect("exact Agent membership adds");
        let member: Value = serde_json::from_str(
            member["content"][0]["text"]
                .as_str()
                .expect("tool response text"),
        )
        .expect("RoleMembership JSON");
        assert_eq!(member["actor_ref"], json!({"kind":"agent","id":agent.id}));
        assert_eq!(member["task_role_id"], created["id"]);
    });
}

#[test]
fn mcp_execution_output_exposes_pr2_authority_fields() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task(&state).await;
        let agent = seed_agent(&state, "PR2 output agent").await;
        let now = now_rfc3339();
        ExecutionRepo::create(
            &*state.db,
            CreateExecution {
                id: "pr2-mcp-execution".to_owned(),
                task_id: task.id.clone(),
                agent_id: Some(agent.id.clone()),
                actor_ref: Some(db::ActorRef::Agent(agent.id.clone())),
                purpose: Some(ExecutionPurpose::Investigate),
                harness_session_id: None,
                role: "implementer".to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("execution creates");

        let page = call_tool(&state, "forge_list_executions", json!({"task_id": task.id})).await;
        let execution = &page["items"][0];
        assert_eq!(
            execution["actor_ref"],
            json!({"kind": "agent", "id": agent.id})
        );
        assert_eq!(execution["purpose"], "investigate");
        assert!(execution["harness_session_id"].is_null());
    });
}

#[test]
fn mcp_task_projection_exposes_lifecycle_without_legacy_recovery_authority() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task_with_legacy_resume_hint(&state, true).await;

        let result = call_tool(
            &state,
            "forge_get_task",
            json!({"task_id": task.id.clone()}),
        )
        .await;

        assert!(result.get("error_annotation").is_none());
        assert!(result["lifecycle"]["state"].is_string());
        assert!(result.get("status").is_none());
        let persisted = TaskRepo::get_by_id(&*state.db, &task.id, false)
            .await
            .expect("task lookup succeeds")
            .expect("task remains persisted");
        let persisted_annotation: Value = serde_json::from_str(
            persisted
                .error_annotation
                .as_deref()
                .expect("annotation persists"),
        )
        .expect("persisted annotation is valid JSON");
        assert!(persisted_annotation["recovery_actions"]
            .as_array()
            .expect("persisted recovery actions")
            .contains(&json!("resume_session")));
    });
}

#[test]
fn mcp_task_list_uses_items_and_lifecycle_fields() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task_with_legacy_resume_hint(&state, false).await;

        let result = call_tool(
            &state,
            "forge_list_tasks",
            json!({"project_id": task.project_id}),
        )
        .await;
        let listed_task = result["items"]
            .as_array()
            .expect("task page")
            .iter()
            .find(|item| item["id"] == task.id)
            .expect("task appears in list");
        assert!(listed_task["lifecycle"]["state"].is_string());
        assert!(listed_task.get("status").is_none());
    });
}

#[test]
fn revised_public_surface_does_not_advertise_retired_collaboration_tools() {
    run_async(async {
        let state = sqlite_state().await;
        let result = dispatch(&state, "tools/list", json!({}))
            .await
            .expect("tools/list succeeds");
        let names = result["tools"]
            .as_array()
            .expect("tools list")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();
        assert!(names.contains(&"forge_list_task_roles"));
        assert!(names.contains(&"forge_list_task_handoffs"));
        assert!(names.contains(&"forge_list_task_review_executions"));
        assert!(names.contains(&"forge_list_validation_runs"));
        assert!(names.iter().all(|name| !name.contains("agent_chat")));
        assert!(names.iter().all(|name| !name.contains("memory")));
        assert!(names.iter().all(|name| !name.contains("main_agent")));
    });
}

#[test]
fn scoped_tools_list_marks_project_id_optional() {
    run_async(async {
        let state = sqlite_state().await;
        let result = dispatch_with_context(
            &state,
            &McpContext {
                project_id: Some("project-1".to_owned()),
                user_id: Some("test-user".to_owned()),
                is_admin: false,
            },
            "tools/list",
            json!({}),
        )
        .await
        .expect("tools/list succeeds");
        let tools = result["tools"]
            .as_array()
            .expect("tools/list returns {tools:[...]}");
        let create_task = tools
            .iter()
            .find(|tool| tool["name"] == "forge_create_task")
            .expect("create task descriptor exists");
        let required = create_task["inputSchema"]["required"]
            .as_array()
            .expect("required is an array");

        assert!(required.iter().any(|value| value == "title"));
        assert!(!required.iter().any(|value| value == "project_id"));

        assert!(tools
            .iter()
            .any(|tool| tool["name"] == "forge_transition_task_lifecycle"));
    });
}

#[test]
fn unknown_method_returns_method_not_found() {
    run_async(async {
        let state = sqlite_state().await;
        let error = dispatch(&state, "unknown", json!({}))
            .await
            .expect_err("unknown method errors");
        assert_eq!(error.code, -32601);
    });
}

#[test]
fn embedded_read_tools_require_authenticated_server_identity() {
    run_async(async {
        let state = sqlite_state().await;
        let error = dispatch_with_context(
            &state,
            &McpContext::default(),
            "tools/call",
            json!({
                "name": "forge_list_agent_profiles",
                "arguments": { "identity_id": "identity-1" }
            }),
        )
        .await
        .expect_err("profile reads must not accept an unbound caller identity");
        assert_eq!(error.code, -32003);

        let error = dispatch(
            &state,
            "tools/call",
            json!({
                "name": "forge_get_main_agent",
                "arguments": {}
            }),
        )
        .await
        .expect_err("removed binding tools are unknown");
        assert_eq!(error.code, -32601);
    });
}

#[test]
fn removed_agent_os_and_semantic_memory_tools_are_not_found() {
    run_async(async {
        let state = sqlite_state().await;
        for name in [
            "forge_set_main_agent",
            "forge_set_project_agent",
            "forge_send_agent_chat_message",
            "forge_create_agent_handoff",
            "forge_memory_search",
            "forge_memory_get",
        ] {
            let error = dispatch(&state, "tools/call", json!({"name": name, "arguments": {}}))
                .await
                .expect_err("retired vertical tools fail closed");
            assert_eq!(error.code, -32601, "{name}");
        }
    });
}

#[test]
fn tools_call_returns_mcp_content_envelope() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task(&state).await;
        let result = dispatch(
            &state,
            "tools/call",
            json!({
                "name": "forge_update_task",
                "arguments": {
                    "task_id": task.id,
                    "title": "Updated MCP",
                    "version": task.version,
                }
            }),
        )
        .await
        .expect("tool succeeds");

        let content = result["content"].as_array().expect("content array");
        assert_eq!(content.len(), 1);
        assert_eq!(content[0]["type"], "text");

        let payload: Value = serde_json::from_str(
            content[0]["text"]
                .as_str()
                .expect("text content is a string"),
        )
        .expect("text content is JSON");
        assert_eq!(payload["title"], "Updated MCP");
    });
}

#[test]
fn scoped_mcp_injects_project_id_for_project_tools() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let result = call_tool_scoped(
            &state,
            &project_id,
            "forge_create_task",
            json!({
                "title": "Scoped task",
                "description": "created through scoped MCP"
            }),
        )
        .await;

        assert_eq!(result["project_id"], project_id);
        assert_eq!(result["title"], "Scoped task");
    });
}

#[test]
fn forge_create_task_persists_validated_task_type() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        for (task_type, title) in [
            ("discovery", "Discovery task"),
            ("planning", "Planning task"),
        ] {
            let result = call_tool(
                &state,
                "forge_create_task",
                json!({
                    "project_id": project_id,
                    "title": title,
                    "type": task_type
                }),
            )
            .await;
            let task_id = result["id"].as_str().expect("task id");
            let task = TaskRepo::get_by_id(&*state.db, task_id, false)
                .await
                .expect("task lookup succeeds")
                .expect("task persists");
            assert_eq!(task.task_type, task_type);
        }
    });
}

#[test]
fn forge_create_task_rejects_invalid_priority_with_field_error() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let error = call_tool_error(
            &state,
            "forge_create_task",
            json!({
                "project_id": project_id,
                "title": "Bad priority",
                "priority": "high"
            }),
        )
        .await;

        assert_eq!(error.code, -32602);
        let data = error.data.expect("error data");
        assert_eq!(data["field"], "priority");
        assert_eq!(data["accepted"]["type"], "integer");
    });
}

#[test]
fn forge_create_task_rejects_invalid_type_with_field_error() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let error = call_tool_error(
            &state,
            "forge_create_task",
            json!({
                "project_id": project_id,
                "title": "Bad type",
                "type": "epic"
            }),
        )
        .await;

        assert_eq!(error.code, -32602);
        let data = error.data.expect("error data");
        assert_eq!(data["field"], "type");
        assert_eq!(
            data["accepted"]["enum"],
            json!([
                "implementation",
                "planning",
                "discovery",
                "review",
                "validation"
            ])
        );
    });
}

#[test]
fn forge_create_task_rejects_missing_project_id_with_field_error() {
    run_async(async {
        let state = sqlite_state().await;
        let error = call_tool_error(
            &state,
            "forge_create_task",
            json!({
                "title": "Missing project"
            }),
        )
        .await;

        assert_eq!(error.code, -32602);
        let data = error.data.expect("error data");
        assert_eq!(data["field"], "project_id");
        assert_eq!(data["accepted"]["type"], "string");
    });
}

#[test]
fn forge_create_task_rejects_unknown_project_with_field_error() {
    run_async(async {
        let state = sqlite_state().await;
        let error = call_tool_error(
            &state,
            "forge_create_task",
            json!({
                "project_id": "does-not-exist",
                "title": "Unknown project"
            }),
        )
        .await;

        assert_eq!(error.code, -32602);
        let data = error.data.expect("error data");
        assert_eq!(data["field"], "project_id");
        assert_eq!(data["accepted"]["constraint"], "existing project id");
    });
}

#[test]
fn forge_create_sub_tasks_rejects_malformed_subtasks() {
    run_async(async {
        let state = sqlite_state().await;
        let root = seed_task(&state).await;
        let error = call_tool_error(
            &state,
            "forge_create_sub_tasks",
            json!({
                "parent_task_id": root.id,
                "subtasks": [{"description": "missing title"}]
            }),
        )
        .await;

        assert_eq!(error.code, -32602);
        let data = error.data.expect("error data");
        assert!(data["details"]
            .as_str()
            .expect("details string")
            .contains("title"));
    });
}

#[test]
fn scoped_mcp_rejects_mismatched_project_id() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let error = dispatch_with_context(
            &state,
            &McpContext {
                project_id: Some(project_id),
                user_id: Some("test-user".to_owned()),
                is_admin: false,
            },
            "tools/call",
            json!({
                "name": "forge_list_tasks",
                "arguments": {
                    "project_id": "other-project"
                }
            }),
        )
        .await
        .expect_err("mismatched project scope errors");

        assert_eq!(error.code, -32602);
    });
}

#[test]
fn scoped_mcp_allows_task_id_tool_for_same_project() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let task = seed_task_in_project(&state, project_id.clone()).await;

        let result = call_tool_scoped(
            &state,
            &project_id,
            "forge_get_task",
            json!({ "task_id": task.id }),
        )
        .await;

        assert_eq!(result["id"], task.id);
        assert_eq!(result["project_id"], project_id);
    });
}

#[test]
fn scoped_mcp_rejects_task_id_tool_for_other_project() {
    run_async(async {
        let state = sqlite_state().await;
        let (scoped_project_id, _) = seed_project_repo(&state).await;
        let other_task = seed_task(&state).await;

        let error = dispatch_with_context(
            &state,
            &McpContext {
                project_id: Some(scoped_project_id),
                user_id: Some("test-user".to_owned()),
                is_admin: false,
            },
            "tools/call",
            json!({
                "name": "forge_get_task",
                "arguments": {
                    "task_id": other_task.id,
                }
            }),
        )
        .await
        .expect_err("cross-project task handle errors");

        assert_eq!(error.code, -32602);
    });
}

#[test]
fn scoped_mcp_rejects_parent_task_id_tool_for_other_project() {
    run_async(async {
        let state = sqlite_state().await;
        let (scoped_project_id, _) = seed_project_repo(&state).await;
        let other_task = seed_task(&state).await;

        let error = dispatch_with_context(
            &state,
            &McpContext {
                project_id: Some(scoped_project_id),
                user_id: Some("test-user".to_owned()),
                is_admin: false,
            },
            "tools/call",
            json!({
                "name": "forge_create_sub_tasks",
                "arguments": {
                    "parent_task_id": other_task.id,
                    "subtasks": [{
                        "title": "Cross-project child",
                        "description": "should not be created"
                    }]
                }
            }),
        )
        .await
        .expect_err("cross-project parent task handle errors");

        assert_eq!(error.code, -32602);
    });
}

#[test]
fn scoped_mcp_rejects_execution_id_tool_for_other_project() {
    run_async(async {
        let state = sqlite_state().await;
        let (scoped_project_id, _) = seed_project_repo(&state).await;
        let other_task = seed_task(&state).await;
        let execution_id = seed_execution(&state, other_task.id).await;

        let error = dispatch_with_context(
            &state,
            &McpContext {
                project_id: Some(scoped_project_id),
                user_id: Some("test-user".to_owned()),
                is_admin: false,
            },
            "tools/call",
            json!({
                "name": "forge_follow_up_execution",
                "arguments": {
                    "execution_id": execution_id,
                    "idempotency_key": "cross-project-follow-up",
                    "message": "continue"
                }
            }),
        )
        .await
        .expect_err("cross-project execution handle errors");

        assert_eq!(error.code, -32602);
    });
}

#[test]
fn user_scoped_mcp_isolates_projects_and_derived_resources() {
    run_async(async {
        let state = sqlite_state().await;
        add_user(&state, "user-b").await;
        let (project_a, _) = seed_private_project_repo(&state, "test-user", "Project A").await;
        let (project_b, _) = seed_private_project_repo(&state, "user-b", "Project B").await;
        let task_a = seed_task_in_project(&state, project_a.clone()).await;
        let task_b = seed_task_in_project(&state, project_b.clone()).await;
        let execution_b = seed_execution(&state, task_b.id.clone()).await;
        let agent_b = seed_private_agent(&state, "user-b").await;
        let global_agent = seed_agent(&state, "global harness").await;
        let (validation_run_b, evidence_b) =
            seed_validation_run_and_evidence(&state, &task_b).await;
        let user_a = user_mcp_context("test-user");
        let user_b = user_mcp_context("user-b");

        let unauthenticated = dispatch_with_context(
            &state,
            &McpContext::default(),
            "tools/call",
            json!({ "name": "forge_create_project", "arguments": { "name": "No owner" } }),
        )
        .await
        .expect_err("Project creation requires an authenticated Human");
        assert_eq!(unauthenticated.code, -32003);

        let projects = call_tool_as(
            &state,
            &user_a,
            "forge_list_projects",
            json!({ "limit": 100 }),
        )
        .await;
        let listed_project_ids = projects["items"]
            .as_array()
            .expect("project items")
            .iter()
            .filter_map(|item| item["id"].as_str())
            .collect::<Vec<_>>();
        assert!(listed_project_ids.contains(&project_a.as_str()));
        assert!(!listed_project_ids.contains(&project_b.as_str()));
        assert!(projects["total_count"].is_null());

        let agents_a = call_tool_as(
            &state,
            &user_a,
            "forge_list_agents",
            json!({ "limit": 100 }),
        )
        .await;
        let agent_items_a = agents_a["items"].as_array().unwrap();
        assert!(!agent_items_a.iter().any(|item| item["id"] == agent_b));
        let global_agent_a = agent_items_a
            .iter()
            .find(|item| item["id"] == global_agent.id)
            .expect("global Agent remains visible");
        assert!(global_agent_a["daemon_id"].is_null());
        let first_agent_page =
            call_tool_as(&state, &user_a, "forge_list_agents", json!({ "limit": 1 })).await;
        assert_eq!(first_agent_page["items"].as_array().unwrap().len(), 1);
        assert_eq!(first_agent_page["has_more"], false);
        assert!(first_agent_page["next_cursor"].is_null());
        let agents_b = call_tool_as(
            &state,
            &user_b,
            "forge_list_agents",
            json!({ "limit": 100 }),
        )
        .await;
        let private_agent_b = agents_b["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["id"] == agent_b)
            .expect("owner sees private Agent");
        assert_eq!(private_agent_b["config_json"]["api_key"], "[redacted]");
        assert_eq!(
            private_agent_b["config_json"]["nested"]["password"],
            "[redacted]"
        );
        assert_eq!(private_agent_b["config_json"]["safe"], "visible");

        let updated_project = call_tool_as(
            &state,
            &user_a,
            "forge_update_project",
            json!({ "project_id": project_a, "name": "Project A updated" }),
        )
        .await;
        assert_eq!(updated_project["name"], "Project A updated");
        let updated_task = call_tool_as(
            &state,
            &user_a,
            "forge_update_task",
            json!({
                "task_id": task_a.id,
                "version": task_a.version,
                "title": "Task A updated"
            }),
        )
        .await;
        assert_eq!(updated_task["title"], "Task A updated");

        let error = call_tool_error_as(
            &state,
            &user_a,
            "forge_get_project",
            json!({ "project_id": project_b }),
        )
        .await;
        assert_eq!(error.code, -32001);
        let error = call_tool_error_as(
            &state,
            &user_a,
            "forge_update_project",
            json!({ "project_id": project_b, "name": "cross-user write" }),
        )
        .await;
        assert_eq!(error.code, -32001);
        let error = call_tool_error_as(
            &state,
            &user_a,
            "forge_update_project_hooks",
            json!({ "project_id": project_b, "version": 1, "project_hooks": [] }),
        )
        .await;
        assert_eq!(error.code, -32001);
        let project_b_after = ProjectRepo::get_by_id(&*state.db, &project_b)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(project_b_after.name, "Project B");

        for (name, arguments) in [
            ("forge_get_task", json!({ "task_id": task_b.id })),
            (
                "forge_update_task",
                json!({ "task_id": task_b.id, "version": task_b.version, "title": "cross-user" }),
            ),
            (
                "forge_transition_task_lifecycle",
                json!({ "task_id": task_b.id, "to_state": "active", "expected_lifecycle_version": 1, "idempotency_key": "cross-user" }),
            ),
            (
                "forge_create_task_role",
                json!({ "task_id": task_b.id, "role": "reviewer", "coordination_mode": "independent" }),
            ),
            (
                "forge_create_task_gate",
                json!({ "task_id": task_b.id, "gate_kind": "cross-user", "policy": { "schema_version": 1, "validations": [] } }),
            ),
            (
                "forge_get_review_execution",
                json!({ "execution_id": execution_b }),
            ),
            (
                "forge_get_validation_run",
                json!({ "validation_run_id": validation_run_b }),
            ),
            ("forge_get_evidence", json!({ "evidence_id": evidence_b })),
        ] {
            let error = call_tool_error_as(&state, &user_a, name, arguments).await;
            assert_eq!(error.code, -32001, "{name}");
        }

        for name in [
            "forge_list_task_dependencies",
            "forge_list_executions",
            "forge_get_task_lifecycle",
            "forge_list_task_lifecycle_transitions",
            "forge_list_task_roles",
            "forge_list_task_review_executions",
            "forge_list_validation_runs",
            "forge_list_task_artifacts",
            "forge_list_task_messages",
            "forge_list_task_handoffs",
            "forge_list_task_proposals",
            "forge_list_task_decisions",
        ] {
            let error =
                call_tool_error_as(&state, &user_a, name, json!({ "task_id": task_b.id })).await;
            assert_eq!(error.code, -32001, "{name}");
        }

        let validation_run = ValidationRunRepo::get_validation_run(&*state.db, &validation_run_b)
            .await
            .unwrap()
            .unwrap();
        let gate_b = call_tool_as(
            &state,
            &user_b,
            "forge_create_task_gate",
            json!({
                "task_id": task_b.id,
                "gate_kind": "authorization-fixture",
                "policy": {
                    "schema_version": 1,
                    "validations": [{
                        "validation_run_id": validation_run_b,
                        "evidence_id": evidence_b,
                        "evidence_digest": "c".repeat(64),
                        "check_identity": validation_run.check_identity,
                        "config_digest": validation_run.config_digest,
                        "workspace_id": validation_run.workspace_id,
                        "commit_sha": validation_run.commit_sha,
                        "workspace_snapshot_digest": validation_run.workspace_snapshot_digest,
                        "required_outcome": "passed"
                    }]
                }
            }),
        )
        .await;
        let gate_id = gate_b["gate"]["id"].as_str().unwrap().to_owned();
        let evaluation_b = call_tool_as(
            &state,
            &user_b,
            "forge_evaluate_gate",
            json!({ "gate_id": gate_id }),
        )
        .await;
        let evaluation_id = evaluation_b["id"].as_str().unwrap().to_owned();
        for (name, arguments) in [
            ("forge_get_gate", json!({ "gate_id": gate_id })),
            ("forge_evaluate_gate", json!({ "gate_id": gate_id })),
            (
                "forge_revise_gate_policy",
                json!({ "gate_id": gate_id, "expected_active_revision": 1, "policy": { "schema_version": 1 } }),
            ),
            (
                "forge_get_gate_evaluation",
                json!({ "evaluation_id": evaluation_id }),
            ),
        ] {
            let error = call_tool_error_as(&state, &user_a, name, arguments).await;
            assert_eq!(error.code, -32001, "{name}");
        }

        let error = call_tool_error_as(
            &state,
            &user_a,
            "forge_start_execution",
            json!({
                "work_unit_id": "foreign-work-unit",
                "expected_work_unit_version": 1,
                "agent_id": agent_b,
                "purpose": "implement",
                "prompt": "must not use another user's private Agent"
            }),
        )
        .await;
        assert_eq!(error.code, -32602);
        let execution_a = seed_execution(&state, task_a.id.clone()).await;
        let error = call_tool_error_as(
            &state,
            &user_a,
            "forge_follow_up_execution",
            json!({
                "execution_id": execution_a,
                "idempotency_key": "wrong-agent-follow-up",
                "message": "continue",
                "agent_id": agent_b
            }),
        )
        .await;
        assert_eq!(error.code, -32004);

        let task_a_read = call_tool_as(
            &state,
            &McpContext {
                project_id: Some(project_a.clone()),
                ..user_a.clone()
            },
            "forge_get_task",
            json!({ "task_id": task_a.id }),
        )
        .await;
        assert_eq!(task_a_read["id"], task_a.id);
        let error = call_tool_error_as(
            &state,
            &McpContext {
                project_id: Some(project_a.clone()),
                ..user_b.clone()
            },
            "forge_get_project",
            json!({ "project_id": project_a }),
        )
        .await;
        assert_eq!(error.code, -32001);

        ProjectMemberRepo::add_member(
            &*state.db,
            CreateProjectMember {
                id: new_uuid_v4(),
                project_id: project_b.clone(),
                user_id: "test-user".to_owned(),
                role: "member".to_owned(),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("User A becomes a Project B member");
        let member_task_read = call_tool_as(
            &state,
            &user_a,
            "forge_get_task",
            json!({ "task_id": task_b.id }),
        )
        .await;
        assert_eq!(member_task_read["id"], task_b.id);
        let error = call_tool_error_as(
            &state,
            &McpContext {
                project_id: Some(project_a.clone()),
                ..user_a.clone()
            },
            "forge_get_task",
            json!({ "task_id": task_b.id }),
        )
        .await;
        assert_eq!(error.code, -32602);

        let created_project = call_tool_as(
            &state,
            &user_a,
            "forge_create_project",
            json!({ "name": "MCP-owned" }),
        )
        .await;
        let created_project_id = created_project["id"].as_str().unwrap();
        let created = ProjectRepo::get_by_id(&*state.db, created_project_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(created.owner_id.as_deref(), Some("test-user"));
        let owner_membership =
            ProjectMemberRepo::get_member(&*state.db, created_project_id, "test-user")
                .await
                .unwrap()
                .expect("owner membership exists");
        assert_eq!(owner_membership.role, "owner");
        let error = call_tool_error_as(
            &state,
            &user_b,
            "forge_get_project",
            json!({ "project_id": created_project_id }),
        )
        .await;
        assert_eq!(error.code, -32001);

        let own_task = call_tool_as(
            &state,
            &user_b,
            "forge_get_task",
            json!({ "task_id": task_b.id }),
        )
        .await;
        assert_eq!(own_task["id"], task_b.id);
    });
}

#[test]
fn mcp_project_visibility_does_not_grant_mutation_authority() {
    run_async(async {
        let state = sqlite_state().await;
        for user_id in ["owner-a", "admin-a", "member-a", "viewer-a", "user-b"] {
            add_user(&state, user_id).await;
        }
        let (project_a, _) = seed_private_project_repo(&state, "owner-a", "Project A").await;
        let (project_b, _) = seed_private_project_repo(&state, "user-b", "Project B").await;
        for (user_id, role) in [
            ("admin-a", "admin"),
            ("member-a", "member"),
            ("viewer-a", "viewer"),
        ] {
            ProjectMemberRepo::add_member(
                &*state.db,
                CreateProjectMember {
                    id: new_uuid_v4(),
                    project_id: project_a.clone(),
                    user_id: user_id.to_owned(),
                    role: role.to_owned(),
                    created_at: now_rfc3339(),
                    updated_at: now_rfc3339(),
                },
            )
            .await
            .expect("Project role membership creates");
        }

        let owner = user_mcp_context("owner-a");
        let admin = user_mcp_context("admin-a");
        let member = user_mcp_context("member-a");
        let viewer = user_mcp_context("viewer-a");
        let outsider = user_mcp_context("user-b");
        let owner_task = call_tool_as(
            &state,
            &owner,
            "forge_create_task",
            json!({"project_id":project_a,"title":"Owner task"}),
        )
        .await;
        let member_task = call_tool_as(
            &state,
            &member,
            "forge_create_task",
            json!({"project_id":project_a,"title":"Member task"}),
        )
        .await;
        let admin_task = call_tool_as(
            &state,
            &admin,
            "forge_create_task",
            json!({"project_id":project_a,"title":"Admin task"}),
        )
        .await;
        assert_eq!(owner_task["project_id"], project_a);
        assert_eq!(member_task["project_id"], project_a);
        assert_eq!(admin_task["project_id"], project_a);

        let project_read = call_tool_as(
            &state,
            &viewer,
            "forge_get_project",
            json!({"project_id":project_a}),
        )
        .await;
        assert_eq!(project_read["id"], project_a);
        let task_read = call_tool_as(
            &state,
            &viewer,
            "forge_get_task",
            json!({"task_id":owner_task["id"]}),
        )
        .await;
        assert_eq!(task_read["id"], owner_task["id"]);

        let updated = call_tool_as(
            &state,
            &admin,
            "forge_update_project",
            json!({"project_id":project_a,"name":"Admin Project update"}),
        )
        .await;
        assert_eq!(updated["name"], "Admin Project update");
        for (context, name, arguments) in [
            (
                &viewer,
                "forge_create_task",
                json!({"project_id":project_a,"title":"Viewer task"}),
            ),
            (
                &viewer,
                "forge_update_task",
                json!({"task_id":owner_task["id"],"version":owner_task["version"],"title":"Viewer update"}),
            ),
            (
                &member,
                "forge_update_project",
                json!({"project_id":project_a,"name":"Member Project update"}),
            ),
            (
                &viewer,
                "forge_update_project",
                json!({"project_id":project_a,"name":"Viewer Project update"}),
            ),
        ] {
            let error = call_tool_error_as(&state, context, name, arguments).await;
            assert_eq!(error.code, -32003, "{name} requires write/admin authority");
        }
        let foreign_read = call_tool_error_as(
            &state,
            &outsider,
            "forge_get_project",
            json!({"project_id":project_a}),
        )
        .await;
        assert_eq!(foreign_read.code, -32001);
        let foreign_write = call_tool_error_as(
            &state,
            &outsider,
            "forge_create_task",
            json!({"project_id":project_a,"title":"Foreign task"}),
        )
        .await;
        assert_eq!(foreign_write.code, -32001);

        let foreign_parent = call_tool_as(
            &state,
            &outsider,
            "forge_create_task",
            json!({"project_id":project_b,"title":"Foreign parent"}),
        )
        .await;
        let local_foreign_parent = call_tool_error_as(
            &state,
            &owner,
            "forge_create_task",
            json!({"project_id":project_a,"title":"Rejected local child","parent_task_id":foreign_parent["id"]}),
        )
        .await;
        assert_eq!(local_foreign_parent.code, -32001);
        let project_scoped = McpContext {
            project_id: Some(project_a.clone()),
            ..owner.clone()
        };
        let scoped_foreign_parent = call_tool_error_as(
            &state,
            &project_scoped,
            "forge_create_task",
            json!({"title":"Rejected scoped child","parent_task_id":foreign_parent["id"]}),
        )
        .await;
        assert_eq!(scoped_foreign_parent.code, -32001);
        let project_a_task_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = ?")
                .bind(&project_a)
                .fetch_one(state.db.pool())
                .await
                .expect("Project A Task count");
        assert_eq!(
            project_a_task_count, 3,
            "failed MCP creates leave no partial Task"
        );

        let now = now_rfc3339();
        let ownerless = ProjectRepo::create(
            &*state.db,
            CreateProject {
                id: new_uuid_v4(),
                name: "System Project".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("ownerless Project creates");
        let public_read = call_tool_as(
            &state,
            &outsider,
            "forge_get_project",
            json!({"project_id":ownerless.id}),
        )
        .await;
        assert_eq!(public_read["id"], ownerless.id);
        for context in [
            outsider.clone(),
            McpContext {
                project_id: Some(ownerless.id.clone()),
                ..outsider.clone()
            },
        ] {
            let error = call_tool_error_as(
                &state,
                &context,
                "forge_create_task",
                json!({"project_id":ownerless.id,"title":"World writable"}),
            )
            .await;
            assert_eq!(error.code, -32003);
        }
        let ownerless_update = call_tool_error_as(
            &state,
            &outsider,
            "forge_update_project",
            json!({"project_id":ownerless.id,"name":"World writable"}),
        )
        .await;
        assert_eq!(ownerless_update.code, -32003);
    });
}

#[test]
fn forge_update_task_updates_mutable_fields() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task(&state).await;
        let parent = call_tool(
            &state,
            "forge_create_task",
            json!({
                "project_id": task.project_id,
                "title": "MCP parent"
            }),
        )
        .await;
        let created_child = call_tool(
            &state,
            "forge_create_task",
            json!({
                "project_id": task.project_id,
                "title": "MCP created child",
                "parent_task_id": parent["id"],
            }),
        )
        .await;
        assert_eq!(created_child["parent_task_id"], parent["id"]);
        assert_eq!(created_child["subtask_order"], 0);
        let result = call_tool(
            &state,
            "forge_update_task",
            json!({
                "task_id": task.id,
                "parent_task_id": parent["id"],
                "title": "Updated MCP",
                "description": "changed",
                "priority": 9,
                "version": task.version,
            }),
        )
        .await;

        assert_eq!(result["title"], "Updated MCP");
        assert_eq!(result["description"], "changed");
        assert_eq!(result["priority"], 9);
        assert_eq!(result["parent_task_id"], parent["id"]);
        assert_eq!(result["subtask_order"], 1);
        assert!(result.get("plan").is_none());
        assert_eq!(result["version"], 2);
        let detached = call_tool(
            &state,
            "forge_update_task",
            json!({
                "task_id": task.id,
                "parent_task_id": null,
                "version": result["version"],
            }),
        )
        .await;
        assert!(detached["parent_task_id"].is_null());
        assert!(detached["subtask_order"].is_null());
        let legacy_plan = dispatch(
            &state,
            "tools/call",
            json!({
                "name": "forge_update_task",
                "arguments": {
                    "task_id": task.id,
                    "plan": "test it",
                    "version": detached["version"],
                },
            }),
        )
        .await
        .expect_err("retired Task plan field is rejected");
        assert_eq!(legacy_plan.code, -32602);
        let legacy_plan_column: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_table_info('task') WHERE name = 'plan'",
        )
        .fetch_one(state.db.pool())
        .await
        .expect("Task schema can be inspected");
        assert_eq!(legacy_plan_column, 0, "V125 physically retires Task.plan");
    });
}

#[test]
fn forge_update_task_rejects_cross_repo_parent_without_partial_write() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task(&state).await;
        let now = now_rfc3339();
        let other_repo = RepoRepo::create(
            &*state.db,
            CreateRepo {
                id: new_uuid_v4(),
                project_id: task.project_id.clone(),
                name: "other-repo".to_owned(),
                remote_url: "https://example.invalid/other-repo".to_owned(),
                local_path: None,
                work_mode: db::WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("second Repo creates");
        let other_parent = TaskRepo::create(
            &*state.db,
            CreateTask {
                id: new_uuid_v4(),
                project_id: task.project_id.clone(),
                repo_id: Some(other_repo.id),
                parent_task_id: None,
                subtask_order: None,
                title: "Parent in other Repo".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                lifecycle_state: db::TaskLifecycleState::Ready,
                is_automation: false,
                priority: 0,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("other Repo parent creates");

        let error = call_tool_error(
            &state,
            "forge_update_task",
            json!({
                "task_id": task.id,
                "parent_task_id": other_parent.id,
                "version": task.version,
            }),
        )
        .await;
        assert!(
            error.message.contains("same Repo"),
            "unexpected error: {error:?}"
        );

        let after = TaskRepo::get_by_id(&*state.db, &task.id, false)
            .await
            .expect("Task query succeeds")
            .expect("Task remains live");
        assert_eq!(after.repo_id, task.repo_id);
        assert_eq!(after.parent_task_id, task.parent_task_id);
        assert_eq!(after.subtask_order, task.subtask_order);
        assert_eq!(after.version, task.version);
    });
}

#[test]
fn forge_transition_task_lifecycle_requires_exact_identity_and_returns_authority() {
    run_async(async {
        let state = sqlite_state().await;
        let task = seed_task(&state).await;
        let unauthenticated = dispatch_with_context(
            &state,
            &McpContext::default(),
            "tools/call",
            json!({
                "name": "forge_transition_task_lifecycle",
                "arguments": {
                    "task_id": task.id,
                    "to_state": "active",
                    "expected_lifecycle_version": task.version,
                    "idempotency_key": "mcp-transition-unauthenticated",
                },
            }),
        )
        .await
        .expect_err("MCP lifecycle transitions require an authenticated Actor");
        assert_eq!(unauthenticated.code, -32003);

        let incomplete_reason = dispatch_with_context(
            &state,
            &McpContext {
                project_id: None,
                user_id: Some("human-actor".to_owned()),
                is_admin: false,
            },
            "tools/call",
            json!({
                "name": "forge_transition_task_lifecycle",
                "arguments": {
                    "task_id": task.id,
                    "to_state": "active",
                    "expected_lifecycle_version": task.version,
                    "idempotency_key": "mcp-transition-unpaired-reason",
                    "reason_kind": "manual_start",
                },
            }),
        )
        .await
        .expect_err("reason provenance must be an exact pair");
        assert_eq!(incomplete_reason.code, -32602);

        let result = dispatch_with_context(
            &state,
            &McpContext {
                project_id: None,
                user_id: Some("human-actor".to_owned()),
                is_admin: false,
            },
            "tools/call",
            json!({
                "name": "forge_transition_task_lifecycle",
                "arguments": {
                    "task_id": task.id,
                    "to_state": "active",
                    "expected_lifecycle_version": task.version,
                    "idempotency_key": "mcp-transition-1",
                    "reason_kind": "manual_start",
                    "reason_ref": "user-action:manual_start",
                },
            }),
        )
        .await
        .expect("authenticated Actor can request a valid transition");
        let result = result["content"][0]["text"]
            .as_str()
            .map(|text| serde_json::from_str::<Value>(text).expect("tool result JSON"))
            .expect("tool text content");

        assert_eq!(result["lifecycle"]["state"], "active");
        assert!(result["transition_id"].is_string());
        assert!(result["lifecycle"]["version"].as_i64().unwrap() >= 2);
        assert!(result.get("status").is_none());
    });
}

#[test]
fn forge_register_agent_registers_agent() {
    run_async(async {
        let state = sqlite_state().await;
        let (executor_type, daemon_id) = seed_agent_registration_deps(&state).await;
        let result = call_tool_as(
            &state,
            &admin_mcp_context(),
            "forge_register_agent",
            json!({
                "name": "codex",
                "executor_type": executor_type.clone(),
                "daemon_id": daemon_id.clone(),
            }),
        )
        .await;

        assert_eq!(result["name"], "codex");
        assert_eq!(result["executor_type"], executor_type);
        assert_eq!(result["daemon_id"], daemon_id);
        assert_eq!(result["status"], "idle");
    });
}

#[test]
fn forge_list_agents_returns_paginated_agents() {
    run_async(async {
        let state = sqlite_state().await;
        let (executor_type, daemon_id) = seed_agent_registration_deps(&state).await;
        call_tool_as(
            &state,
            &admin_mcp_context(),
            "forge_register_agent",
            json!({
                "name": "codex",
                "executor_type": executor_type.clone(),
                "daemon_id": daemon_id.clone(),
            }),
        )
        .await;

        let result = call_tool_as(
            &state,
            &admin_mcp_context(),
            "forge_list_agents",
            json!({
                "status": "idle",
                "limit": 10,
            }),
        )
        .await;
        let agents = result["items"].as_array().expect("agents array");
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0]["name"], "codex");
        assert_eq!(result["has_more"], false);
    });
}

#[test]
fn forge_list_projects_returns_paginated_projects() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let result = call_tool(
            &state,
            "forge_list_projects",
            json!({
                "limit": 10,
            }),
        )
        .await;
        let projects = result["items"].as_array().expect("projects array");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0]["id"], project_id);
        assert_eq!(projects[0]["name"], "Forge");
    });
}

#[test]
fn forge_create_project_creates_project() {
    run_async(async {
        let state = sqlite_state().await;
        let result = call_tool(
            &state,
            "forge_create_project",
            json!({
                "name": "MCP",
            }),
        )
        .await;

        assert_eq!(result["name"], "MCP");
        assert!(result["id"].as_str().is_some());
        assert!(result.get("settings").is_none());
        assert_eq!(result["paused"], false);
    });
}

#[test]
fn forge_get_project_returns_project_details() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let result = call_tool(
            &state,
            "forge_get_project",
            json!({
                "project_id": project_id,
            }),
        )
        .await;

        assert_eq!(result["name"], "Forge");
        assert!(result.get("settings").is_none());
        assert_eq!(result["paused"], false);
        assert!(result.get("project_hooks").is_some());
    });
}

#[test]
fn forge_update_project_updates_mutable_fields() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let result = call_tool(
            &state,
            "forge_update_project",
            json!({
                "project_id": project_id,
                "name": "Updated Forge",
                "paused": true
            }),
        )
        .await;

        assert_eq!(result["name"], "Updated Forge");
        assert!(result.get("settings").is_none());
        assert_eq!(result["paused"], true);
        assert!(result["paused_at"].as_str().is_some());
    });
}

#[test]
fn forge_update_project_hooks_replaces_rules_at_exact_version() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let project = ProjectRepo::get_by_id(&*state.db, &project_id)
            .await
            .unwrap()
            .unwrap();

        let result = call_tool(
            &state,
            "forge_update_project_hooks",
            json!({
                "project_id": project_id,
                "version": project.version,
                "project_hooks": [{
                    "id": "all-work-notify",
                    "enabled": true,
                    "name": "Notify after all work completes",
                    "trigger": { "type": "project.all_work_completed" },
                    "filters": null,
                    "action": { "type": "notify", "title": "Complete", "message": "All work completed", "severity": "info" },
                    "cooldown_seconds": 0,
                    "max_concurrent_runs": 1
                }]
            }),
        )
        .await;

        assert_eq!(result["project_hooks"][0]["id"], "all-work-notify");
        assert!(result.get("settings").is_none());
    });
}

#[test]
fn forge_update_project_hooks_requires_supported_trigger() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let project = ProjectRepo::get_by_id(&*state.db, &project_id)
            .await
            .unwrap()
            .unwrap();
        let error = dispatch(
            &state,
            "tools/call",
            json!({
                "name": "forge_update_project_hooks",
                "arguments": {
                    "project_id": project_id,
                    "version": project.version,
                    "project_hooks": [{
                        "id": "unsupported-trigger",
                        "enabled": true,
                        "name": "Unsupported",
                        "trigger": { "type": "task.stuck" },
                        "filters": null,
                        "action": { "type": "notify", "title": "x", "message": "y", "severity": null },
                        "cooldown_seconds": null,
                        "max_concurrent_runs": 1
                    }]
                }
            }),
        )
        .await
        .expect_err("unsupported hook trigger error");

        assert_eq!(error.code, -32602);
    });
}

#[test]
fn retired_project_lifecycle_tool_is_not_translated() {
    run_async(async {
        let state = sqlite_state().await;
        let (project_id, _) = seed_project_repo(&state).await;
        let before = ProjectRepo::get_by_id(&*state.db, &project_id)
            .await
            .unwrap()
            .unwrap();
        let error = dispatch(
            &state,
            "tools/call",
            json!({
                "name": "forge_update_project_lifecycle_hooks",
                "arguments": { "project_id": project_id, "lifecycle_hooks": {} }
            }),
        )
        .await
        .expect_err("retired MCP tool is not found");
        assert_eq!(error.code, -32601);
        let after = ProjectRepo::get_by_id(&*state.db, &project_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(after.version, before.version);
        assert_eq!(after.project_hooks_json, before.project_hooks_json);
    });
}

#[test]
fn forge_create_sub_tasks_creates_subtasks() {
    run_async(async {
        let state = sqlite_state().await;
        let root = seed_task(&state).await;
        let result = call_tool(
            &state,
            "forge_create_sub_tasks",
            json!({
                "parent_task_id": root.id.clone(),
                "subtasks": [
                    { "title": "One" },
                    { "title": "Two" },
                    { "title": "Three" }
                ]
            }),
        )
        .await;

        let subtasks = result["subtasks"].as_array().expect("subtasks array");
        assert_eq!(subtasks.len(), 3);
        for (index, subtask) in subtasks.iter().enumerate() {
            assert_eq!(subtask["subtask_order"], index as i64);
        }
    });
}

#[test]
fn forge_create_sub_tasks_nested_rejected() {
    run_async(async {
        let state = sqlite_state().await;
        let root = seed_task(&state).await;
        let now = now_rfc3339();
        let subtask = TaskRepo::create(
            &*state.db,
            CreateTask {
                id: new_uuid_v4(),
                project_id: root.project_id.clone(),
                repo_id: root.repo_id.clone(),
                parent_task_id: Some(root.id.clone()),
                title: "Existing subtask".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                lifecycle_state: db::TaskLifecycleState::Ready,
                is_automation: false,
                priority: 0,
                subtask_order: Some(0),
                merge_config: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("subtask creates");

        let error = dispatch(
            &state,
            "tools/call",
            json!({
                "name": "forge_create_sub_tasks",
                "arguments": {
                    "parent_task_id": subtask.id,
                    "subtasks": [
                        { "title": "Nested" }
                    ]
                }
            }),
        )
        .await
        .expect_err("nested subtask errors");
        let response = error.into_response(json!(1));
        let error = response.error.expect("error response");
        assert_eq!(error.code, -32602);
        assert_eq!(
            error
                .data
                .as_ref()
                .and_then(|data| data.get("code"))
                .and_then(Value::as_str),
            Some("NESTED_SUBTASK_UNSUPPORTED")
        );
    });
}

#[test]
fn uncertain_execution_start_maps_to_a_structured_no_retry_error() {
    let response =
        crate::error::McpToolError::from(services::ServiceError::ExecutionStartUncertain {
            execution_id: "execution-uncertain".to_owned(),
        })
        .into_response(json!(1));
    let error = response.error.expect("uncertain Start maps to MCP error");
    assert_eq!(error.code, -32029);
    assert!(error.message.contains("will not be retried automatically"));
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str),
        Some("execution_start_uncertain")
    );
    assert_eq!(
        error
            .data
            .as_ref()
            .and_then(|data| data.get("execution_id"))
            .and_then(Value::as_str),
        Some("execution-uncertain")
    );
}
