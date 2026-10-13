use crate::Result;
use db::{
    new_uuid_v4, now_rfc3339, AgentListQuery, AgentRepo, AgentStatus, CreateAgent, CreateProject,
    CreateRepo, CreateRoleMembership, CreateTask, CreateTaskRole, DaemonRepo, DaemonStatus,
    PageRequest, Project, ProjectRepo, Repo, RepoRepo, RoleMembershipRepo, SortBy, SortOrder,
    SqliteDb, TaskLifecycleState, TaskRepo, TaskRoleRepo, UpdateDaemonReport, UpdateTask,
    UpsertDaemon,
};

struct DemoTask {
    title: &'static str,
    description: &'static str,
    lifecycle_state: TaskLifecycleState,
    priority: i64,
    error_annotation: Option<&'static str>,
}

const DEMO_TASKS: &[DemoTask] = &[
    DemoTask {
        title: "Set up CI pipeline",
        description: "Configure GitHub Actions for lint, test, and build.",
        lifecycle_state: TaskLifecycleState::Backlog,
        priority: 3,
        error_annotation: None,
    },
    DemoTask {
        title: "Add rate limiting to API",
        description: "Implement token-bucket rate limiting on public endpoints.",
        lifecycle_state: TaskLifecycleState::Backlog,
        priority: 5,
        error_annotation: None,
    },
    DemoTask {
        title: "Design database schema for notifications",
        description: "Create tables for user notification preferences and delivery log.",
        lifecycle_state: TaskLifecycleState::Ready,
        priority: 7,
        error_annotation: None,
    },
    DemoTask {
        title: "Implement user settings page",
        description: "Build the frontend settings UI with theme toggle and profile editing.",
        lifecycle_state: TaskLifecycleState::Ready,
        priority: 5,
        error_annotation: None,
    },
    DemoTask {
        title: "Write integration tests for auth flow",
        description: "Cover login, token refresh, and logout with end-to-end tests.",
        lifecycle_state: TaskLifecycleState::Ready,
        priority: 6,
        error_annotation: None,
    },
    DemoTask {
        title: "Refactor error handling middleware",
        description: "Unify error responses across all API routes with proper status codes.",
        lifecycle_state: TaskLifecycleState::Active,
        priority: 8,
        error_annotation: None,
    },
    DemoTask {
        title: "Add WebSocket support for live updates",
        description: "Implement real-time task status notifications via WebSocket.",
        lifecycle_state: TaskLifecycleState::Active,
        priority: 7,
        error_annotation: None,
    },
    DemoTask {
        title: "Optimize database queries for task listing",
        description: "Add indexes and rewrite slow queries identified in profiling.",
        lifecycle_state: TaskLifecycleState::Blocked,
        priority: 6,
        error_annotation: None,
    },
    DemoTask {
        title: "Fix pagination cursor encoding",
        description: "Cursor was double-encoding base64, causing 400 errors on page 2+.",
        lifecycle_state: TaskLifecycleState::Blocked,
        priority: 9,
        error_annotation: None,
    },
    DemoTask {
        title: "Add dark mode support",
        description: "Implement CSS custom properties for theme switching.",
        lifecycle_state: TaskLifecycleState::Done,
        priority: 4,
        error_annotation: None,
    },
    DemoTask {
        title: "Set up logging infrastructure",
        description: "Structured JSON logging with request ID propagation.",
        lifecycle_state: TaskLifecycleState::Done,
        priority: 8,
        error_annotation: None,
    },
    DemoTask {
        title: "Initial project scaffolding",
        description: "Workspace structure, CI config, and dev tooling.",
        lifecycle_state: TaskLifecycleState::Done,
        priority: 10,
        error_annotation: None,
    },
    DemoTask {
        title: "Migrate user sessions to Redis",
        description: "Move session storage from in-memory to Redis for horizontal scaling.",
        lifecycle_state: TaskLifecycleState::Blocked,
        priority: 6,
        error_annotation: Some(
            r#"{"type":"workspace_reset_required","message":"workspace reset required: task branch no longer exists in repo"}"#,
        ),
    },
    DemoTask {
        title: "Upgrade OpenAPI spec generator",
        description: "Update codegen tooling to v4 for better TypeScript output.",
        lifecycle_state: TaskLifecycleState::Blocked,
        priority: 4,
        error_annotation: Some(
            r#"{"type":"workspace_error","message":"workspace error: repository directory not found on disk"}"#,
        ),
    },
];

pub async fn install_demo_data(db: &SqliteDb) -> Result<()> {
    let now = now_rfc3339();
    let project = find_or_create_demo_project(db, &now).await?;
    let repo = find_or_create_demo_repo(db, &project.id, &now).await?;
    let agent_id = find_or_create_null_agent(db, &now).await?;
    install_demo_daemon(db, &now).await?;
    install_demo_tasks(db, &project.id, &repo.id, &agent_id, &now).await?;

    tracing::info!("demo data installed");
    Ok(())
}

async fn find_or_create_demo_project(db: &SqliteDb, now: &str) -> Result<Project> {
    let projects = ProjectRepo::list(db, page_request()).await?;
    if let Some(project) = projects
        .items
        .into_iter()
        .find(|project| project.name == "Demo")
    {
        return Ok(project);
    }

    ProjectRepo::create(
        db,
        CreateProject {
            id: new_uuid_v4(),
            name: "Demo".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .map_err(Into::into)
}

async fn find_or_create_demo_repo(db: &SqliteDb, project_id: &str, now: &str) -> Result<Repo> {
    let repos = RepoRepo::list_by_project(db, project_id, page_request()).await?;
    if let Some(repo) = repos
        .items
        .into_iter()
        .find(|repo| repo.name == "demo-repo" && repo.remote_url == "https://example.com/demo.git")
    {
        return Ok(repo);
    }

    RepoRepo::create(
        db,
        CreateRepo {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            name: "demo-repo".to_owned(),
            remote_url: "https://example.com/demo.git".to_owned(),
            local_path: None,
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .map_err(Into::into)
}

async fn find_or_create_null_agent(db: &SqliteDb, now: &str) -> Result<String> {
    let page = AgentRepo::list(
        db,
        AgentListQuery {
            status: None,
            executor_type: Some("null".to_owned()),
            capabilities: Vec::new(),
            harness_only: false,
            page: page_request(),
        },
    )
    .await?;
    if let Some(agent) = page.items.into_iter().find(|a| a.is_default) {
        return Ok(agent.id);
    }

    let agent = AgentRepo::create(
        db,
        CreateAgent {
            id: new_uuid_v4(),
            name: "Null Default".to_owned(),
            description: None,
            executor_type: "null".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: "[]".to_owned(),
            config_json: r#"{"delay_seconds":5}"#.to_owned(),
            credential_ref: None,
            daemon_id: None,
            max_concurrent_tasks: 10,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: true,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            prompt_template: None,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await?;
    Ok(agent.id)
}

async fn install_demo_daemon(db: &SqliteDb, now: &str) -> Result<()> {
    let daemon = DaemonRepo::upsert_by_machine_id(
        db,
        UpsertDaemon {
            id: new_uuid_v4(),
            machine_id: "demo".to_owned(),
            hostname: "demo".to_owned(),
            os: "demo".to_owned(),
            arch: "demo".to_owned(),
            agent_version: None,
            labels_json: r#"{"demo":"true"}"#.to_owned(),
            status: DaemonStatus::Online,
            registration_token_hash: None,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await?;

    DaemonRepo::update_report(
        db,
        UpdateDaemonReport {
            id: daemon.id,
            last_report_at: now.to_owned(),
            status: DaemonStatus::Online,
            detected_clis_json: r#"[{"kind":"null","availability":"authenticated","config_path":null,"version":null,"path":null}]"#.to_owned(),
            labels_json: Some(r#"{"demo":"true"}"#.to_owned()),
            updated_at: now.to_owned(),
        },
    )
    .await?;

    Ok(())
}

async fn install_demo_tasks(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    agent_id: &str,
    now: &str,
) -> Result<()> {
    let existing = TaskRepo::list(
        db,
        db::TaskListQuery {
            project_id: project_id.to_owned(),
            q: None,
            lifecycle_states: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_cancelled: false,
            include_deleted: false,
            page: page_request(),
        },
    )
    .await?;

    let existing_titles: Vec<&str> = existing.items.iter().map(|t| t.title.as_str()).collect();

    for (i, demo_task) in DEMO_TASKS.iter().enumerate() {
        if existing_titles.contains(&demo_task.title) {
            continue;
        }

        let is_active = demo_task.lifecycle_state == TaskLifecycleState::Active;

        let task_id = new_uuid_v4();
        TaskRepo::create(
            db,
            CreateTask {
                id: task_id.clone(),
                project_id: project_id.to_owned(),
                repo_id: Some(repo_id.to_owned()),
                parent_task_id: None,
                subtask_order: None,
                // Demo data uses the same membership writer as normal Task
                // creation.  The task-level fields are a projection, not an
                // independent assignment source.
                title: demo_task.title.to_owned(),
                description: Some(demo_task.description.to_owned()),
                task_type: "implementation".to_owned(),
                lifecycle_state: demo_task.lifecycle_state,
                is_automation: false,
                priority: demo_task.priority,
                merge_config: None,
                created_at: now.to_owned(),
                updated_at: now.to_owned(),
            },
        )
        .await?;

        if is_active {
            let task_role_id = new_uuid_v4();
            TaskRoleRepo::create(
                db,
                CreateTaskRole {
                    id: task_role_id.clone(),
                    task_id: task_id.clone(),
                    role: "implementer".to_owned(),
                    coordination_mode: None,
                    policy_json: "{}".to_owned(),
                    created_at: now.to_owned(),
                    updated_at: now.to_owned(),
                },
            )
            .await?;
            RoleMembershipRepo::add(
                db,
                CreateRoleMembership {
                    id: new_uuid_v4(),
                    task_role_id,
                    actor_kind: db::ActorKind::Agent,
                    actor_id: agent_id.to_owned(),
                    status: db::RoleMembershipStatus::Active,
                    created_at: now.to_owned(),
                    updated_at: now.to_owned(),
                },
            )
            .await?;
        }

        if let Some(annotation) = demo_task.error_annotation {
            TaskRepo::update(
                db,
                UpdateTask {
                    id: task_id,
                    expected_version: 1,
                    title: None,
                    description: None,
                    priority: None,
                    merge_config: None,
                    error_annotation: Some(Some(annotation.to_owned())),
                    blocked_json: None,
                    failed_json: None,
                    parent_task_id: None,
                    updated_at: now.to_owned(),
                },
            )
            .await?;
        }

        let _position = (i + 1) as f64;
    }

    Ok(())
}

fn page_request() -> PageRequest {
    PageRequest {
        cursor: None,
        limit: 1_000,
        include_total: false,
        sort_by: SortBy::Id,
        sort_order: SortOrder::Asc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{create_sqlite_pool, run_migrations};

    async fn test_db() -> SqliteDb {
        let pool = create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        run_migrations(&pool).await.expect("migrations run");
        SqliteDb::new(pool)
    }

    #[tokio::test]
    async fn test_install_demo_data_idempotent() {
        let db = test_db().await;

        install_demo_data(&db)
            .await
            .expect("first install succeeds");
        install_demo_data(&db)
            .await
            .expect("second install succeeds");

        let task_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task")
            .fetch_one(db.pool())
            .await
            .expect("task count succeeds");

        assert_eq!(task_count, DEMO_TASKS.len() as i64);
    }

    #[tokio::test]
    async fn test_demo_creates_null_agent() {
        let db = test_db().await;
        install_demo_data(&db).await.expect("install succeeds");

        let agents = AgentRepo::list(
            &db,
            AgentListQuery {
                status: None,
                executor_type: Some("null".to_owned()),
                capabilities: Vec::new(),
                harness_only: false,
                page: page_request(),
            },
        )
        .await
        .expect("agent list succeeds");

        assert_eq!(agents.items.len(), 1);
        assert_eq!(agents.items[0].name, "Null Default");
    }

    #[tokio::test]
    async fn test_demo_tasks_spread_across_states() {
        let db = test_db().await;
        install_demo_data(&db).await.expect("install succeeds");

        let project = ProjectRepo::list(&db, page_request())
            .await
            .expect("list projects");
        let project_id = &project.items[0].id;

        for status in [
            "backlog",
            "todo",
            "in_progress",
            "review",
            "done",
            "blocked",
        ] {
            let tasks = TaskRepo::list(
                &db,
                db::TaskListQuery {
                    project_id: project_id.clone(),
                    q: None,
                    lifecycle_states: Vec::new(),
                    agent_ids: Vec::new(),
                    assignee_types: Vec::new(),
                    assignee_ids: Vec::new(),
                    priority: None,
                    include_cancelled: false,
                    include_deleted: false,
                    page: page_request(),
                },
            )
            .await
            .expect("list tasks");
            assert!(
                !tasks.items.is_empty(),
                "expected tasks in state '{status}'"
            );
        }
    }
}
