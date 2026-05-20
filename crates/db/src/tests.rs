use crate::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, validate_uuid_v4, AgentListQuery,
    AgentRepo, AgentStatus, AgentTaskListQuery, ArchiveTask, ClaimTask, ConversationListQuery,
    ConversationMessageListQuery, ConversationMessageRepo, ConversationMessageRole,
    ConversationMessageStatus, ConversationRepo, ConversationStatus, CreateAgent,
    CreateConversation, CreateConversationMessage, CreateExecution, CreateProject,
    CreateProjectAgentLink, CreateProjectMember, CreateRepo, CreateReview, CreateSkill, CreateTask,
    CreateTaskRoleAssignment, CreateWorkspace, DaemonRepo, DaemonStatus, DbError, ExecutionRepo,
    ExecutionStatus, NotificationListQuery, NotificationRepo, PageRequest, ProjectAgentLinkRepo,
    ProjectMemberRepo, ProjectRepo, RepoRepo, ReviewRepo, ReviewStatus, SkillRepo, SortBy,
    SortOrder, SqliteDb, Task, TaskDependencyRepo, TaskListQuery, TaskRepo, TaskRoleAssignmentRepo,
    UpdateAgent, UpdateConversation, UpdateExecution, UpdateProject, UpdateRepo, UpdateSkill,
    UpdateTask, UpsertDaemon, WorkMode, WorkspaceRepo, WorkspaceStatus,
};
use crate::{RefreshToken, RefreshTokenRepo, User, UserRepo};

fn page(limit: i64) -> PageRequest {
    PageRequest {
        cursor: None,
        limit,
        include_total: true,
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Asc,
    }
}

async fn sqlite_db() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");
    SqliteDb::new(pool)
}

async fn seed_daemon(db: &SqliteDb) -> String {
    let now = now_rfc3339();
    let daemon_id = new_uuid_v4();
    DaemonRepo::upsert_by_machine_id(
        db,
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
    daemon_id
}

async fn seed_project_repo_agent(db: &SqliteDb) -> (String, String, String) {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let agent_id = new_uuid_v4();
    let daemon_id = seed_daemon(db).await;

    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_string(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "forge".to_owned(),
            remote_url: "https://example.com/forge.git".to_owned(),
            local_path: Some("/tmp/forge-test-repo".to_owned()),
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo creates");
    ProjectRepo::update(
        db,
        UpdateProject {
            id: project_id.clone(),
            name: None,
            settings: None,
            primary_repo_id: Some(Some(repo_id.clone())),
            paused_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("project primary repo updates");
    AgentRepo::create(
        db,
        CreateAgent {
            id: agent_id.clone(),
            name: "shell".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: r#"["rust"]"#.to_owned(),
            config_json: "{}".to_owned(),
            daemon_id: Some(daemon_id),
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
            updated_at: now.clone(),
        },
    )
    .await
    .expect("agent creates");

    (project_id, repo_id, agent_id)
}

async fn seed_project(db: &SqliteDb, name: &str, owner_id: Option<String>) -> String {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: name.to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_owned(),
            primary_repo_id: None,
            owner_id,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("project creates");
    project_id
}

async fn seed_agent(
    db: &SqliteDb,
    name: &str,
    visibility: &str,
    owner_id: Option<String>,
) -> String {
    let now = now_rfc3339();
    let agent_id = new_uuid_v4();
    AgentRepo::create(
        db,
        CreateAgent {
            id: agent_id.clone(),
            name: name.to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            daemon_id: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id,
            visibility: visibility.to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("agent creates");
    agent_id
}

async fn seed_project_agent_link(
    db: &SqliteDb,
    project_id: &str,
    agent_id: &str,
    linked_by_user_id: &str,
) -> String {
    let now = now_rfc3339();
    let link_id = new_uuid_v4();
    ProjectAgentLinkRepo::create(
        db,
        CreateProjectAgentLink {
            id: link_id.clone(),
            project_id: project_id.to_owned(),
            agent_id: agent_id.to_owned(),
            linked_by_user_id: linked_by_user_id.to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("project agent link creates");
    link_id
}

#[tokio::test]
async fn test_project_agent_link_uniqueness() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let linked_by_user_id = new_uuid_v4();
    let project_id = seed_project(&db, "Agent links", None).await;
    let agent_id = seed_agent(&db, "linked agent", "global", None).await;

    ProjectAgentLinkRepo::create(
        &db,
        CreateProjectAgentLink {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            agent_id: agent_id.clone(),
            linked_by_user_id: linked_by_user_id.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("first project agent link creates");

    let duplicate = ProjectAgentLinkRepo::create(
        &db,
        CreateProjectAgentLink {
            id: new_uuid_v4(),
            project_id,
            agent_id,
            linked_by_user_id,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await;
    assert!(
        matches!(duplicate, Err(DbError::Check(_))),
        "duplicate link should fail with a uniqueness check error"
    );
}

#[tokio::test]
async fn test_project_agent_link_cascade_on_project_delete() {
    let db = sqlite_db().await;
    let linked_by_user_id = new_uuid_v4();
    let project_id = seed_project(&db, "Project cascade link", None).await;
    let agent_id = seed_agent(&db, "project cascade agent", "global", None).await;
    seed_project_agent_link(&db, &project_id, &agent_id, &linked_by_user_id).await;

    ProjectRepo::delete(&db, &project_id)
        .await
        .expect("project deletes");

    let link = ProjectAgentLinkRepo::get_by_project_and_agent(&db, &project_id, &agent_id)
        .await
        .expect("project agent link lookup succeeds");
    assert!(link.is_none());
    let links = ProjectAgentLinkRepo::list_by_project(&db, &project_id)
        .await
        .expect("project agent links list");
    assert!(links.is_empty());
    let agent = AgentRepo::get_by_id(&db, &agent_id)
        .await
        .expect("agent lookup succeeds");
    assert!(agent.is_some());
}

#[tokio::test]
async fn test_project_agent_link_cascade_on_agent_delete() {
    let db = sqlite_db().await;
    let linked_by_user_id = new_uuid_v4();
    let project_id = seed_project(&db, "Agent cascade link", None).await;
    let agent_id = seed_agent(&db, "agent cascade agent", "global", None).await;
    seed_project_agent_link(&db, &project_id, &agent_id, &linked_by_user_id).await;

    AgentRepo::delete(&db, &agent_id)
        .await
        .expect("agent deletes");

    let link = ProjectAgentLinkRepo::get_by_project_and_agent(&db, &project_id, &agent_id)
        .await
        .expect("project agent link lookup succeeds");
    assert!(link.is_none());
    let links = ProjectAgentLinkRepo::list_by_project(&db, &project_id)
        .await
        .expect("project agent links list");
    assert!(links.is_empty());
    let project = ProjectRepo::get_by_id(&db, &project_id)
        .await
        .expect("project lookup succeeds");
    assert!(project.is_some());
}

#[tokio::test]
async fn test_list_agents_usable_in_project() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let u1_id = seed_user(&db).await;
    let u2_id = seed_user(&db).await;
    let global_agent_id = seed_agent(&db, "global agent", "global", None).await;
    let u1_agent_id = seed_agent(&db, "u1 account agent", "account", Some(u1_id.clone())).await;
    let u2_agent_id = seed_agent(&db, "u2 account agent", "account", Some(u2_id.clone())).await;
    let project_id = seed_project(&db, "Usable agents", Some(u1_id.clone())).await;

    ProjectMemberRepo::add_member(
        &db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            user_id: u1_id.clone(),
            role: "owner".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("u1 project member creates");
    ProjectMemberRepo::add_member(
        &db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            user_id: u2_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("u2 project member creates");

    let usable = SqliteDb::list_agents_usable_in_project(&db, &project_id, &u1_id)
        .await
        .expect("usable agents list");
    assert!(usable
        .iter()
        .any(|agent| agent.id.as_str() == global_agent_id.as_str()));
    assert!(usable
        .iter()
        .any(|agent| agent.id.as_str() == u1_agent_id.as_str()));
    assert!(!usable
        .iter()
        .any(|agent| agent.id.as_str() == u2_agent_id.as_str()));

    ProjectAgentLinkRepo::create(
        &db,
        CreateProjectAgentLink {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            agent_id: u2_agent_id.clone(),
            linked_by_user_id: u1_id.clone(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("u2 account agent links to project");

    let usable = SqliteDb::list_agents_usable_in_project(&db, &project_id, &u1_id)
        .await
        .expect("usable agents list after link");
    assert!(usable
        .iter()
        .any(|agent| agent.id.as_str() == global_agent_id.as_str()));
    assert!(usable
        .iter()
        .any(|agent| agent.id.as_str() == u1_agent_id.as_str()));
    assert!(usable
        .iter()
        .any(|agent| agent.id.as_str() == u2_agent_id.as_str()));
}

async fn seed_task(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    agent_id: Option<&str>,
    status: String,
    title: &str,
) -> String {
    let now = now_rfc3339();
    let task_id = new_uuid_v4();
    TaskRepo::create(
        db,
        CreateTask {
            id: task_id.clone(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: title.to_owned(),
            description: None,
            task_type: "task".to_owned(),
            status,
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            plan: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    if let Some(agent_id) = agent_id {
        TaskRoleAssignmentRepo::assign(
            db,
            CreateTaskRoleAssignment {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                role_name: "coder".to_owned(),
                assignee_type: Some(crate::AssigneeKind::Agent),
                assignee_id: Some(agent_id.to_owned()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("role assignment creates");
    }
    task_id
}

async fn seed_ordered_task(
    db: &SqliteDb,
    project_id: &str,
    repo_id: &str,
    parent_task_id: Option<&str>,
    subtask_order: Option<i64>,
    title: &str,
    created_at: &str,
) -> Task {
    TaskRepo::create(
        db,
        CreateTask {
            id: new_uuid_v4(),
            project_id: project_id.to_owned(),
            repo_id: Some(repo_id.to_owned()),
            parent_task_id: parent_task_id.map(str::to_owned),
            subtask_order,
            assignee_type: None,
            assignee_id: None,
            title: title.to_owned(),
            description: None,
            task_type: "task".to_owned(),
            status: "todo".to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            plan: None,
            created_at: created_at.to_owned(),
            updated_at: created_at.to_owned(),
        },
    )
    .await
    .expect("ordered task creates")
}

#[tokio::test]
async fn task_list_hides_cancelled_and_archived_by_default() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let visible_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_owned(),
        "Visible",
    )
    .await;
    let cancelled_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "cancelled".to_owned(),
        "Cancelled",
    )
    .await;
    let archived_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "done".to_owned(),
        "Archived",
    )
    .await;
    let archived = TaskRepo::get_by_id(&db, &archived_id, false)
        .await
        .unwrap()
        .unwrap();
    TaskRepo::archive(
        &db,
        ArchiveTask {
            id: archived_id.clone(),
            expected_version: archived.version,
            archived_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .unwrap();

    let default_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id: project_id.clone(),
            q: None,
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .unwrap();
    let default_ids = default_page
        .items
        .iter()
        .map(|task| task.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(default_ids, vec![visible_id.as_str()]);

    let included_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id,
            q: None,
            statuses: Vec::new(),
            agent_ids: vec![agent_id],
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: true,
            include_cancelled: true,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .unwrap();
    let included_ids = included_page
        .items
        .iter()
        .map(|task| task.id.as_str())
        .collect::<Vec<_>>();
    assert!(included_ids.contains(&visible_id.as_str()));
    assert!(included_ids.contains(&cancelled_id.as_str()));
    assert!(included_ids.contains(&archived_id.as_str()));
}

#[tokio::test]
async fn task_list_filters_by_user_assignee() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let human_task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Human task",
    )
    .await;
    let agent_task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_owned(),
        "Agent task",
    )
    .await;
    TaskRoleAssignmentRepo::assign(
        &db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: human_task_id.clone(),
            role_name: "coder".to_owned(),
            assignee_type: Some(crate::AssigneeKind::User),
            assignee_id: Some("human".to_owned()),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .unwrap();

    let user_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id: project_id.clone(),
            q: None,
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: vec!["user".to_owned()],
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .unwrap();
    assert_eq!(user_page.items.len(), 1);
    assert_eq!(user_page.items[0].id, human_task_id);

    let human_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id,
            q: None,
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: vec!["user".to_owned()],
            assignee_ids: vec!["human".to_owned()],
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .unwrap();
    assert_eq!(human_page.items.len(), 1);
    assert_eq!(human_page.items[0].id, human_task_id);
    assert_ne!(human_page.items[0].id, agent_task_id);
}

#[tokio::test]
async fn task_list_filters_by_search_query() {
    let db = sqlite_db().await;
    let (project_id, repo_id, _agent_id) = seed_project_repo_agent(&db).await;
    let alpha_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Alpha release",
    )
    .await;
    let description_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Description only",
    )
    .await;
    let percent_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "100% literal",
    )
    .await;
    let wildcard_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "100x wildcard",
    )
    .await;
    seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Beta rollout",
    )
    .await;

    let description_task = TaskRepo::get_by_id(&db, &description_id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    TaskRepo::update(
        &db,
        UpdateTask {
            id: description_id.clone(),
            expected_version: description_task.version,
            title: None,
            description: Some(Some("Needle lives in this description".to_owned())),
            priority: None,
            merge_config: None,
            plan: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("description updates");

    let title_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id: project_id.clone(),
            q: Some("release".to_owned()),
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .expect("search by title");
    assert_eq!(title_page.items.len(), 1);
    assert_eq!(title_page.items[0].id, alpha_id);

    let description_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id: project_id.clone(),
            q: Some("needle".to_owned()),
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .expect("search by description");
    assert_eq!(description_page.items.len(), 1);
    assert_eq!(description_page.items[0].id, description_id);

    let literal_page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id,
            q: Some("100%".to_owned()),
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: page(10),
        },
    )
    .await
    .expect("search escapes wildcards");
    assert_eq!(literal_page.items.len(), 1);
    assert_eq!(literal_page.items[0].id, percent_id);
    assert_ne!(literal_page.items[0].id, wildcard_id);
}

#[tokio::test]
async fn migration_creates_schema_and_enforces_foreign_keys() {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations run");

    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let agent_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let execution_id = new_uuid_v4();
    let review_id = new_uuid_v4();
    let db = SqliteDb::new(pool.clone());
    let daemon_id = seed_daemon(&db).await;

    assert!(validate_uuid_v4(&project_id));

    sqlx::query(
        "INSERT INTO project (id, name, settings, created_at, updated_at) VALUES (?, ?, '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind("Forge")
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("project inserts");

    sqlx::query("INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
    .bind(&repo_id)
    .bind(&project_id)
    .bind("forge")
    .bind("https://example.com/forge.git")
    .bind("/tmp/forge-test-repo")
    .bind("direct_merge")
    .bind("main")
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("repo inserts");

    sqlx::query(
        "INSERT INTO agent (id, name, executor_type, daemon_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&agent_id)
    .bind("shell")
    .bind("shell")
    .bind(&daemon_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("agent inserts");

    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&repo_id)
    .bind("Build DB foundation")
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("task inserts");

    sqlx::query(
        "INSERT INTO execution (id, task_id, agent_id, role, status, logs_path, created_at, updated_at) VALUES (?, ?, ?, 'executor', 'running', ?, ?, ?)",
    )
    .bind(&execution_id)
    .bind(&task_id)
    .bind(&agent_id)
    .bind(format!("sessions/{task_id}/processes/{execution_id}.jsonl"))
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("execution inserts");

    sqlx::query(
        "INSERT INTO review (id, task_id, execution_id, attempt_number, status, step_results_json, started_at, created_at, updated_at) VALUES (?, ?, ?, 1, 'running', '[]', ?, ?, ?)",
    )
    .bind(&review_id)
    .bind(&task_id)
    .bind(&execution_id)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("review inserts");

    let bad_execution_result = sqlx::query(
        "INSERT INTO execution (id, task_id, created_at, updated_at) VALUES (?, ?, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(new_uuid_v4())
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(bad_execution_result.is_err());

    let bad_review_result = sqlx::query(
        "INSERT INTO review (id, task_id, execution_id, attempt_number, created_at, updated_at) VALUES (?, ?, ?, 1, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(&task_id)
    .bind(new_uuid_v4())
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(bad_review_result.is_err());
}

#[tokio::test]
async fn delete_lifecycle_foreign_keys_match_repository_operations() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Delete lifecycle",
    )
    .await;
    let now = now_rfc3339();
    let workspace_id = new_uuid_v4();

    WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: workspace_id.clone(),
            task_id: task_id.clone(),
            repo_id: repo_id.clone(),
            worktree_path: "/tmp/forge-delete-lifecycle".to_owned(),
            branch: workspace::task_branch_name(&task_id),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("workspace creates");

    let execution = ExecutionRepo::create(
        &db,
        CreateExecution {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            agent_id: Some(agent_id.clone()),
            role: "executor".to_owned(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: Some(workspace_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("execution creates");

    sqlx::query(
        "INSERT INTO transition_log (id, task_id, from_state, to_state, trigger_name, triggered_by, trigger_reason, created_at) VALUES (?, ?, 'todo', 'in_progress', NULL, 'system', 'test', ?)",
    )
    .bind(new_uuid_v4())
    .bind(&task_id)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("transition log creates");

    WorkspaceRepo::delete(&db, &workspace_id)
        .await
        .expect("workspace delete clears execution link");
    let execution = ExecutionRepo::get_by_id(&db, &execution.id)
        .await
        .expect("execution loads")
        .expect("execution remains");
    assert_eq!(execution.workspace_id, None);

    ProjectRepo::delete(&db, &project_id)
        .await
        .expect("project delete cascades through task data");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM task WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("task count"),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transition_log WHERE task_id = ?")
            .bind(&task_id)
            .fetch_one(db.pool())
            .await
            .expect("transition count"),
        0
    );

    let (project_id, repo_id, _agent_id) = seed_project_repo_agent(&db).await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Repo cascade",
    )
    .await;
    RepoRepo::delete(&db, &repo_id)
        .await
        .expect("repo delete cascades task data");
    assert!(TaskRepo::get_by_id(&db, &task_id, true)
        .await
        .expect("task lookup succeeds")
        .is_none());
}

#[tokio::test]
async fn sqlite_repo_create_round_trips_local_path() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_string(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");

    let repo = RepoRepo::create(
        &db,
        CreateRepo {
            id: repo_id,
            project_id,
            name: "forge".to_owned(),
            remote_url: "https://example.com/forge.git".to_owned(),
            local_path: Some("/tmp/forge-test-repo".to_owned()),
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("local repo creates");

    assert_eq!(repo.work_mode, WorkMode::DirectMerge);
    assert_eq!(repo.local_path, Some("/tmp/forge-test-repo".to_owned()));
    assert_eq!(repo.remote_url, "https://example.com/forge.git");
}

#[tokio::test]
async fn sqlite_repo_create_round_trips_remote_url() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_string(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");

    let repo = RepoRepo::create(
        &db,
        CreateRepo {
            id: repo_id,
            project_id,
            name: "forge".to_owned(),
            remote_url: "https://example.com/forge.git".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("remote repo creates");

    assert_eq!(repo.work_mode, WorkMode::DirectMerge);
    assert_eq!(repo.local_path, None);
    assert_eq!(repo.remote_url, "https://example.com/forge.git");
}

#[tokio::test]
async fn sqlite_repo_create_rejects_missing_remote_url() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let project_id = new_uuid_v4();

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Forge".to_owned(),
            settings: "{}".to_owned(),
            workflow_definition: "{}".to_string(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project creates");

    let result = sqlx::query("INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
        .bind(new_uuid_v4())
        .bind(project_id)
        .bind("forge")
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(WorkMode::DirectMerge.to_string())
        .bind("main")
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .map_err(crate::DbError::from);

    assert!(matches!(result, Err(DbError::Sqlx(_))));
}

#[tokio::test]
async fn sqlite_execution_role_auditor_round_trips() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "review".to_string(),
        "Audit me",
    )
    .await;
    let execution_id = new_uuid_v4();

    let created = ExecutionRepo::create(
        &db,
        CreateExecution {
            id: execution_id.clone(),
            task_id: task_id.clone(),
            agent_id: Some(agent_id),
            role: "auditor".to_string(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
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
    .expect("auditor execution creates");

    assert_eq!(created.role, "auditor".to_string());

    let loaded = ExecutionRepo::get_by_id(&db, &execution_id)
        .await
        .expect("auditor execution loads")
        .expect("auditor execution exists");
    assert_eq!(loaded.role, "auditor".to_string());
    assert_eq!(
        ExecutionRepo::list_by_task(&db, &task_id, page(10))
            .await
            .expect("executions list")
            .items
            .first()
            .map(|execution| &execution.role),
        Some(&"auditor".to_string())
    );
}

#[tokio::test]
async fn migration_runner_is_idempotent() {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");

    run_migrations(&pool).await.expect("first run succeeds");
    run_migrations(&pool).await.expect("second run succeeds");

    let applied_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("migration count loads");
    let expected_count = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .expect("migration directory reads")
        .filter(|entry| {
            entry
                .as_ref()
                .ok()
                .and_then(|entry| entry.path().file_name()?.to_str().map(str::to_owned))
                .is_some_and(|filename| filename.starts_with('V') && filename.ends_with(".sql"))
        })
        .count() as i64;
    assert_eq!(applied_count, expected_count);
}

#[tokio::test]
async fn notification_repo_crud_and_cascade_delete() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, _agent_id) = seed_project_repo_agent(&db).await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Notification target",
    )
    .await;

    let notification = NotificationRepo::create(
        &db,
        crate::CreateNotification {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            task_id: Some(task_id.clone()),
            event_type: "task.blocked".to_owned(),
            title: "Task blocked".to_owned(),
            body: Some("Need input".to_owned()),
            read: false,
            created_at: now.clone(),
        },
    )
    .await
    .expect("notification creates");
    assert!(!notification.read);

    let page = NotificationRepo::list(
        &db,
        NotificationListQuery {
            project_id: Some(project_id.clone()),
            read: Some(false),
            page: page(20),
        },
    )
    .await
    .expect("notifications list");
    assert_eq!(page.items.len(), 1);

    assert_eq!(
        NotificationRepo::unread_count(&db, Some(&project_id))
            .await
            .expect("unread count"),
        1
    );

    let marked = NotificationRepo::mark_read(&db, &notification.id)
        .await
        .expect("mark read");
    assert!(marked.read);
    assert_eq!(
        NotificationRepo::unread_count(&db, Some(&project_id))
            .await
            .expect("unread count after mark read"),
        0
    );

    NotificationRepo::create(
        &db,
        crate::CreateNotification {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            task_id: Some(task_id.clone()),
            event_type: "review.failed".to_owned(),
            title: "Review failed".to_owned(),
            body: None,
            read: false,
            created_at: now.clone(),
        },
    )
    .await
    .expect("second notification creates");
    NotificationRepo::create(
        &db,
        crate::CreateNotification {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            task_id: Some(task_id.clone()),
            event_type: "merge.failed".to_owned(),
            title: "Merge failed".to_owned(),
            body: Some("conflict".to_owned()),
            read: false,
            created_at: now,
        },
    )
    .await
    .expect("third notification creates");

    assert_eq!(
        NotificationRepo::mark_all_read(&db, Some(&project_id))
            .await
            .expect("mark all read"),
        2
    );
    assert_eq!(
        NotificationRepo::unread_count(&db, Some(&project_id))
            .await
            .expect("unread count after mark all"),
        0
    );

    NotificationRepo::delete(&db, &notification.id)
        .await
        .expect("notification delete");

    let remaining_before_cascade: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notification WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("count notifications before cascade");
    assert_eq!(remaining_before_cascade, 2);

    sqlx::query("DELETE FROM task WHERE id = ?")
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("hard delete task");
    let remaining_after_task_delete: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notification WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("count notifications after task delete");
    assert_eq!(remaining_after_task_delete, 0);

    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_owned(),
        "Notification target 2",
    )
    .await;
    NotificationRepo::create(
        &db,
        crate::CreateNotification {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            task_id: Some(task_id),
            event_type: "task.done".to_owned(),
            title: "Done".to_owned(),
            body: None,
            read: false,
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("notification for project cascade");

    ProjectRepo::delete(&db, &project_id)
        .await
        .expect("project delete cascade");
    let remaining_after_project_delete: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM notification WHERE project_id = ?")
            .bind(&project_id)
            .fetch_one(db.pool())
            .await
            .expect("count notifications after project delete");
    assert_eq!(remaining_after_project_delete, 0);
}

#[tokio::test]
async fn sqlite_repositories_create_update_list_and_get_logs() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;

    let project = ProjectRepo::update(
        &db,
        UpdateProject {
            id: project_id.clone(),
            name: Some("Forge DB".to_owned()),
            settings: None,
            primary_repo_id: None,
            paused_at: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project updates");
    assert_eq!(project.name, "Forge DB");

    let repo = RepoRepo::update(
        &db,
        UpdateRepo {
            id: repo_id.clone(),
            name: None,
            local_path: None,
            remote_url: None,
            work_mode: None,
            default_branch: Some("trunk".to_owned()),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo updates");
    assert_eq!(repo.default_branch, "trunk");

    let skill_id = new_uuid_v4();
    SkillRepo::create(
        &db,
        CreateSkill {
            id: skill_id.clone(),
            project_id: project_id.clone(),
            name: "Rust".to_owned(),
            content: "Use cargo test".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("skill creates");
    let skill = SkillRepo::update(
        &db,
        UpdateSkill {
            id: skill_id,
            name: Some("SQLite".to_owned()),
            content: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("skill updates");
    assert_eq!(skill.name, "SQLite");

    let agent = AgentRepo::update(
        &db,
        UpdateAgent {
            id: agent_id.clone(),
            expected_version: 1,
            name: Some("codex".to_owned()),
            description: None,
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: None,
            config_json: None,
            daemon_id: None,
            max_concurrent_tasks: None,
            heartbeat_interval_seconds: None,
            max_missed_heartbeats: None,
            status: Some(AgentStatus::Busy),
            last_heartbeat_at: Some(Some(now.clone())),
            is_default: None,
            paused: None,
            prompt_template: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("agent updates");
    assert_eq!(agent.version, 2);

    let task_id = new_uuid_v4();
    TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id: project_id.clone(),
            repo_id: Some(repo_id.clone()),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "Implement repo".to_owned(),
            description: Some("SQLite".to_owned()),
            task_type: "task".to_owned(),
            status: "todo".to_string(),
            is_automation: false,
            priority: 10,
            task_state_config: None,
            merge_config: None,
            plan: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    TaskRoleAssignmentRepo::assign(
        &db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            role_name: "coder".to_owned(),
            assignee_type: Some(crate::AssigneeKind::Agent),
            assignee_id: Some(agent_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("role assignment creates");
    let task = TaskRepo::update(
        &db,
        UpdateTask {
            id: task_id.clone(),
            expected_version: 1,
            title: Some("Implement SQLite repo".to_owned()),
            description: None,
            priority: Some(20),
            merge_config: None,
            plan: Some(Some("Map rows manually".to_owned())),
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task updates");
    assert_eq!(task.version, 2);

    let execution_id = new_uuid_v4();
    ExecutionRepo::create(
        &db,
        CreateExecution {
            id: execution_id.clone(),
            task_id: task_id.clone(),
            agent_id: Some(agent_id.clone()),
            role: "executor".to_string(),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some("initial prompt".to_owned()),
            logs_path: Some("logs/run.jsonl".to_owned()),
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
    .expect("execution creates");
    let execution = ExecutionRepo::update(
        &db,
        UpdateExecution {
            id: execution_id.clone(),
            status: Some(ExecutionStatus::Completed),
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            agent_session_id: Some(Some("session".to_owned())),
            agent_message_id: None,
            last_activity_at: None,
            summary: Some(Some("done".to_owned())),
            logs_path: None,
            before_sha: None,
            after_sha: Some(Some("abc123".to_owned())),
            error: None,
            executor_config_snapshot_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("execution updates");
    assert_eq!(execution.status, ExecutionStatus::Completed);
    assert_eq!(execution.prompt.as_deref(), Some("initial prompt"));
    assert_eq!(execution.summary.as_deref(), Some("done"));
    assert_eq!(
        ExecutionRepo::get_logs_path(&db, &execution_id)
            .await
            .expect("logs path loads"),
        Some("logs/run.jsonl".to_owned())
    );

    let review_id = new_uuid_v4();
    ReviewRepo::create(
        &db,
        CreateReview {
            id: review_id.clone(),
            task_id: task_id.clone(),
            execution_id: execution_id.clone(),
            attempt_number: 1,
            status: ReviewStatus::Running,
            step_results_json: "[]".to_owned(),
            started_at: now.clone(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("review creates");
    assert_eq!(
        ReviewRepo::next_attempt_number(&db, &task_id)
            .await
            .expect("next attempt loads"),
        2
    );
    let review = ReviewRepo::update_status(
        &db,
        &review_id,
        ReviewStatus::Passed,
        r#"[{"index":0,"exit_code":0}]"#.to_owned(),
        Some(now.clone()),
        &now,
    )
    .await
    .expect("review updates");
    assert_eq!(review.status, ReviewStatus::Passed);
    assert_eq!(review.step_results_json, r#"[{"index":0,"exit_code":0}]"#);
    assert_eq!(review.finished_at, Some(now.clone()));

    let workspace_id = new_uuid_v4();
    let workspace = WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: workspace_id.clone(),
            task_id: task_id.clone(),
            repo_id: repo_id.clone(),
            worktree_path: "/tmp/forge/worktrees/task/forge".to_owned(),
            branch: ::workspace::task_branch_name(&task_id),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("workspace creates");
    assert_eq!(workspace.cleanup_after, None);
    let cleanup_after = "2026-04-14T01:00:00Z".to_owned();
    let workspace =
        WorkspaceRepo::set_cleanup_after(&db, &workspace_id, Some(cleanup_after.clone()), &now)
            .await
            .expect("cleanup deadline sets");
    assert_eq!(workspace.cleanup_after, Some(cleanup_after));
    let pending = WorkspaceRepo::list_pending_cleanup(&db, "2026-04-14T02:00:00Z")
        .await
        .expect("pending cleanup lists");
    assert_eq!(pending.len(), 1);
    let workspace = WorkspaceRepo::mark_cleaned(&db, &workspace_id, &now)
        .await
        .expect("workspace marks cleaned");
    assert_eq!(workspace.status, WorkspaceStatus::Cleaned);
    assert_eq!(workspace.cleanup_after, None);

    assert_eq!(
        ProjectRepo::list(&db, page(10)).await.unwrap().total_count,
        Some(1)
    );
    assert_eq!(
        RepoRepo::list_by_project(&db, &project_id, page(10))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        SkillRepo::list_by_project(&db, &project_id, page(10))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        AgentRepo::list(
            &db,
            AgentListQuery {
                status: Some(AgentStatus::Busy),
                executor_type: None,
                capabilities: vec!["rust".to_owned()],
                page: page(10),
            },
        )
        .await
        .unwrap()
        .total_count,
        Some(1)
    );
    assert_eq!(
        TaskRepo::list(
            &db,
            TaskListQuery {
                project_id,
                q: None,
                statuses: vec!["todo".to_string()],
                agent_ids: vec![agent_id],
                assignee_types: Vec::new(),
                assignee_ids: Vec::new(),
                priority: Some(20),
                include_archived: false,
                include_cancelled: false,
                include_deleted: false,
                page: page(10),
            },
        )
        .await
        .unwrap()
        .total_count,
        Some(1)
    );
    assert_eq!(
        ExecutionRepo::list_by_task(&db, &task_id, page(10))
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        ReviewRepo::list_by_task(&db, &task_id).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn workspace_task_id_unique_is_preserved() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_owned(),
        "Workspace owner",
    )
    .await;

    WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            repo_id: repo_id.clone(),
            worktree_path: "/tmp/forge/worktrees/task/one".to_owned(),
            branch: format!("task/{task_id}"),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("workspace creates");

    let duplicate = WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id,
            repo_id,
            worktree_path: "/tmp/forge/worktrees/task/two".to_owned(),
            branch: "task/duplicate".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await;

    assert!(duplicate.is_err());
}

#[tokio::test]
async fn next_subtask_order_appends_after_existing_siblings() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let parent_task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_owned(),
        "Parent",
    )
    .await;

    assert_eq!(
        TaskRepo::next_subtask_order(&db, &parent_task_id)
            .await
            .expect("next order loads"),
        0
    );
    let first_order = TaskRepo::next_subtask_order(&db, &parent_task_id)
        .await
        .expect("first order loads");
    seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(first_order),
        "First",
        &now,
    )
    .await;
    assert_eq!(
        TaskRepo::next_subtask_order(&db, &parent_task_id)
            .await
            .expect("next order loads"),
        1
    );
    let second_order = TaskRepo::next_subtask_order(&db, &parent_task_id)
        .await
        .expect("second order loads");
    seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(second_order),
        "Second",
        &now,
    )
    .await;

    assert_eq!(
        TaskRepo::next_subtask_order(&db, &parent_task_id)
            .await
            .expect("next order loads"),
        2
    );
}

#[tokio::test]
async fn list_subtasks_ordered_uses_subtask_order_before_tiebreakers() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let parent_task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_owned(),
        "Parent",
    )
    .await;

    seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(2),
        "Third",
        "2026-04-18T00:00:00Z",
    )
    .await;
    seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(0),
        "First",
        "2026-04-18T00:02:00Z",
    )
    .await;
    seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(1),
        "Second",
        "2026-04-18T00:01:00Z",
    )
    .await;

    let titles = TaskRepo::list_subtasks_ordered(&db, &parent_task_id)
        .await
        .expect("subtasks list")
        .into_iter()
        .map(|task| task.title)
        .collect::<Vec<_>>();

    assert_eq!(titles, vec!["First", "Second", "Third"]);
}

#[tokio::test]
async fn reorder_subtasks_persists_and_rejects_invalid_orders() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let parent_task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_owned(),
        "Parent",
    )
    .await;
    let first = seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(0),
        "First",
        "2026-04-18T00:00:00Z",
    )
    .await;
    let second = seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        Some(&parent_task_id),
        Some(1),
        "Second",
        "2026-04-18T00:01:00Z",
    )
    .await;

    let reordered_at = "2026-04-18T00:02:00Z";
    TaskRepo::reorder_subtasks(
        &db,
        &parent_task_id,
        &[second.id.clone(), first.id.clone()],
        reordered_at,
    )
    .await
    .expect("subtasks reorder");

    let ordered = TaskRepo::list_subtasks_ordered(&db, &parent_task_id)
        .await
        .expect("subtasks list");
    assert_eq!(ordered[0].id, second.id);
    assert_eq!(ordered[0].subtask_order, Some(0));
    assert_eq!(ordered[0].updated_at, reordered_at);
    assert_eq!(ordered[1].id, first.id);
    assert_eq!(ordered[1].subtask_order, Some(1));

    let unknown = TaskRepo::reorder_subtasks(
        &db,
        &parent_task_id,
        &[ordered[0].id.clone(), new_uuid_v4()],
        "2026-04-18T00:03:00Z",
    )
    .await;
    assert!(matches!(unknown, Err(DbError::NotFound)));

    let mismatched_length = TaskRepo::reorder_subtasks(
        &db,
        &parent_task_id,
        &[ordered[0].id.clone()],
        "2026-04-18T00:04:00Z",
    )
    .await;
    assert!(matches!(mismatched_length, Err(DbError::InvalidTransition)));
}

#[tokio::test]
async fn task_reorder_updates_board_position() {
    let db = sqlite_db().await;
    let (project_id, repo_id, _agent_id) = seed_project_repo_agent(&db).await;
    let first = seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        None,
        None,
        "First",
        "2026-04-18T00:00:00Z",
    )
    .await;
    let second = seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        None,
        None,
        "Second",
        "2026-04-18T00:01:00Z",
    )
    .await;

    assert_eq!(first.board_position, 1.0);
    assert_eq!(second.board_position, 2.0);

    let reordered_at = "2026-04-18T00:02:00Z";
    let reordered = TaskRepo::reorder_task(&db, &first.id, 5.5, reordered_at)
        .await
        .expect("task reorders");
    assert_eq!(reordered.board_position, 5.5);
    assert_eq!(reordered.updated_at, reordered_at);

    let loaded = TaskRepo::get_by_id(&db, &first.id, false)
        .await
        .expect("task loads")
        .expect("task exists");
    assert_eq!(loaded.board_position, 5.5);
}

#[tokio::test]
async fn task_list_orders_equal_board_positions_by_created_at() {
    let db = sqlite_db().await;
    let (project_id, repo_id, _agent_id) = seed_project_repo_agent(&db).await;
    let later = seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        None,
        None,
        "Later",
        "2026-04-18T00:01:00Z",
    )
    .await;
    let earlier = seed_ordered_task(
        &db,
        &project_id,
        &repo_id,
        None,
        None,
        "Earlier",
        "2026-04-18T00:00:00Z",
    )
    .await;
    for task_id in [&later.id, &earlier.id] {
        sqlx::query("UPDATE task SET board_position = 10.0 WHERE id = ?")
            .bind(task_id)
            .execute(db.pool())
            .await
            .expect("board position updates");
    }

    let page = TaskRepo::list(
        &db,
        TaskListQuery {
            project_id,
            q: None,
            statuses: Vec::new(),
            agent_ids: Vec::new(),
            assignee_types: Vec::new(),
            assignee_ids: Vec::new(),
            priority: None,
            include_archived: false,
            include_cancelled: false,
            include_deleted: false,
            page: PageRequest {
                cursor: None,
                limit: 10,
                include_total: false,
                sort_by: SortBy::BoardPosition,
                sort_order: SortOrder::Asc,
            },
        },
    )
    .await
    .expect("tasks list");

    assert_eq!(page.items[0].id, earlier.id);
    assert_eq!(page.items[1].id, later.id);
}

#[tokio::test]
async fn sqlite_repositories_enforce_versions_transitions_claims_and_cursors() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let task_id = new_uuid_v4();
    TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: Some(repo_id),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "Claim me".to_owned(),
            description: None,
            task_type: "task".to_owned(),
            status: "todo".to_string(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            plan: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    TaskRoleAssignmentRepo::assign(
        &db,
        CreateTaskRoleAssignment {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            role_name: "coder".to_owned(),
            assignee_type: Some(crate::AssigneeKind::Agent),
            assignee_id: Some(agent_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("role assignment creates");

    let bad_update = TaskRepo::update(
        &db,
        UpdateTask {
            id: task_id.clone(),
            expected_version: 99,
            title: Some("stale".to_owned()),
            description: None,
            priority: None,
            merge_config: None,
            plan: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now.clone(),
        },
    )
    .await;
    assert!(matches!(bad_update, Err(DbError::VersionConflict)));

    let mut tx = db.pool().begin().await.expect("transaction starts");
    let claimed = TaskRepo::claim(
        &db,
        &mut tx,
        ClaimTask {
            task_id: task_id.clone(),
            assignee_type: "agent".to_owned(),
            assignee_id: Some(agent_id.clone()),
            expected_version: 1,
            source_status: "todo".to_owned(),
            target_status: "in_progress".to_owned(),
            capacity_statuses: vec![
                "in_progress".to_owned(),
                "review".to_owned(),
                "merging".to_owned(),
            ],
            execution: CreateExecution {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                agent_id: Some(agent_id.clone()),
                role: "executor".to_string(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
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
            max_concurrent_tasks: 1,
            claimed_at: now.clone(),
        },
    )
    .await
    .expect("task claims");
    tx.commit().await.expect("claim commits");
    assert_eq!(claimed.task.status, "in_progress".to_string());
    assert_eq!(
        AgentRepo::count_active_tasks(&db, &agent_id).await.unwrap(),
        1
    );

    let invalid_cursor = ProjectRepo::list(
        &db,
        PageRequest {
            cursor: Some("not-base64-json".to_owned()),
            limit: 10,
            include_total: false,
            sort_by: SortBy::Id,
            sort_order: SortOrder::Asc,
        },
    )
    .await;
    assert!(matches!(invalid_cursor, Err(DbError::InvalidCursor)));
}

#[tokio::test]
async fn agent_active_task_count_uses_workflow_state_kinds() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let workflow = serde_json::json!({
        "states": [
            { "name": "todo", "kind": "initial" },
            { "name": "running", "kind": "active" },
            { "name": "waiting_review", "kind": "gate" },
            { "name": "done", "kind": "terminal" }
        ]
    });

    sqlx::query("UPDATE project SET workflow_definition = ? WHERE id = ?")
        .bind(workflow.to_string())
        .bind(&project_id)
        .execute(db.pool())
        .await
        .expect("workflow updates");

    seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "running".to_owned(),
        "custom active state",
    )
    .await;
    seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "waiting_review".to_owned(),
        "custom gate state",
    )
    .await;
    seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "done".to_owned(),
        "terminal state",
    )
    .await;

    assert_eq!(
        AgentRepo::count_active_tasks(&db, &agent_id).await.unwrap(),
        2
    );
}

#[tokio::test]
async fn agent_task_list_uses_execution_history() {
    let db = sqlite_db().await;
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let executed_task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "done".to_owned(),
        "executed task",
    )
    .await;
    seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "done".to_owned(),
        "assigned only task",
    )
    .await;
    let now = now_rfc3339();
    ExecutionRepo::create(
        &db,
        CreateExecution {
            id: new_uuid_v4(),
            task_id: executed_task_id.clone(),
            agent_id: Some(agent_id.clone()),
            role: "coder".to_owned(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: Some(now.clone()),
            parent_execution_id: None,
            agent_session_id: None,
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

    let page = TaskRepo::list_by_executing_agent(
        &db,
        AgentTaskListQuery {
            agent_id,
            include_archived: false,
            include_cancelled: true,
            include_deleted: false,
            page: PageRequest {
                cursor: None,
                limit: 10,
                include_total: true,
                sort_by: SortBy::UpdatedAt,
                sort_order: SortOrder::Desc,
            },
        },
    )
    .await
    .expect("agent task list loads");

    assert_eq!(page.total_count, Some(1));
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, executed_task_id);
}

#[tokio::test]
async fn task_claim_rejects_active_entry_barrier() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let task_id = new_uuid_v4();
    let task = TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: Some(repo_id),
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title: "Barrier claim".to_owned(),
            description: None,
            task_type: "task".to_owned(),
            status: "todo".to_owned(),
            is_automation: false,
            priority: 0,
            task_state_config: None,
            merge_config: None,
            plan: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task creates");
    let task = TaskRepo::set_entry_barrier(
        &db,
        &task.id,
        task.version,
        Some(
            r#"{"state":"todo","status":"running","started_at":"2026-04-28T00:00:00Z"}"#.to_owned(),
        ),
        &now,
    )
    .await
    .expect("barrier sets");

    let mut tx = db.pool().begin().await.expect("transaction starts");
    let result = TaskRepo::claim(
        &db,
        &mut tx,
        ClaimTask {
            task_id: task_id.clone(),
            assignee_type: "agent".to_owned(),
            assignee_id: Some(agent_id.clone()),
            expected_version: task.version,
            source_status: "todo".to_owned(),
            target_status: "in_progress".to_owned(),
            capacity_statuses: vec!["in_progress".to_owned()],
            execution: CreateExecution {
                id: new_uuid_v4(),
                task_id,
                agent_id: Some(agent_id),
                role: "executor".to_string(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
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
            max_concurrent_tasks: 1,
            claimed_at: now_rfc3339(),
        },
    )
    .await;

    assert!(matches!(result, Err(DbError::InvalidTransition)));
}

#[tokio::test]
async fn test_add_dependency_success() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let dependency_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_string(),
        "Dependency",
    )
    .await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_string(),
        "Dependent",
    )
    .await;

    TaskDependencyRepo::add_dependency(&db, &task_id, &dependency_id, &now)
        .await
        .expect("dependency adds");

    assert_eq!(
        TaskDependencyRepo::list_dependencies(&db, &task_id)
            .await
            .expect("dependencies list"),
        vec![dependency_id.clone()]
    );
    assert_eq!(
        TaskDependencyRepo::list_dependents(&db, &dependency_id)
            .await
            .expect("dependents list"),
        vec![task_id]
    );
}

#[tokio::test]
async fn test_add_dependency_cycle_rejected() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let first_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "todo".to_string(),
        "First",
    )
    .await;
    let second_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_string(),
        "Second",
    )
    .await;

    TaskDependencyRepo::add_dependency(&db, &second_id, &first_id, &now)
        .await
        .expect("initial dependency adds");
    let result = TaskDependencyRepo::add_dependency(&db, &first_id, &second_id, &now).await;

    assert!(matches!(result, Err(DbError::CycleDetected)));
}

#[tokio::test]
async fn test_dependency_gate_blocks_non_context_holder() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, context_agent_id) = seed_project_repo_agent(&db).await;
    let other_agent_id = new_uuid_v4();
    let other_daemon_id = seed_daemon(&db).await;
    AgentRepo::create(
        &db,
        CreateAgent {
            id: other_agent_id.clone(),
            name: "other".to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            daemon_id: Some(other_daemon_id),
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
            updated_at: now.clone(),
        },
    )
    .await
    .expect("second agent creates");
    let dependency_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&context_agent_id),
        "review".to_string(),
        "Dependency",
    )
    .await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_string(),
        "Dependent",
    )
    .await;
    ExecutionRepo::create(
        &db,
        CreateExecution {
            id: new_uuid_v4(),
            task_id: dependency_id.clone(),
            agent_id: Some(context_agent_id),
            role: "executor".to_string(),
            status: ExecutionStatus::Completed,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
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
    .expect("dependency execution creates");
    TaskDependencyRepo::add_dependency(&db, &task_id, &dependency_id, &now)
        .await
        .expect("dependency adds");

    let mut tx = db.pool().begin().await.expect("transaction starts");
    let result = TaskRepo::claim(
        &db,
        &mut tx,
        ClaimTask {
            task_id: task_id.clone(),
            assignee_type: "agent".to_owned(),
            assignee_id: Some(other_agent_id.clone()),
            expected_version: 1,
            source_status: "todo".to_owned(),
            target_status: "in_progress".to_owned(),
            capacity_statuses: vec![
                "in_progress".to_owned(),
                "review".to_owned(),
                "merging".to_owned(),
            ],
            execution: CreateExecution {
                id: new_uuid_v4(),
                task_id,
                agent_id: Some(other_agent_id),
                role: "executor".to_string(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
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
            max_concurrent_tasks: 1,
            claimed_at: now,
        },
    )
    .await;

    assert!(matches!(result, Err(DbError::DependencyGate)));
}

#[tokio::test]
async fn test_unsatisfied_dependencies_empty_when_done() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let (project_id, repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let dependency_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "done".to_string(),
        "Dependency",
    )
    .await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "todo".to_string(),
        "Dependent",
    )
    .await;
    TaskDependencyRepo::add_dependency(&db, &task_id, &dependency_id, &now)
        .await
        .expect("dependency adds");

    assert!(TaskDependencyRepo::unsatisfied_dependencies(&db, &task_id)
        .await
        .expect("unsatisfied dependencies list")
        .is_empty());
}

#[tokio::test]
async fn conversation_crud_and_message_sequence() {
    let db = sqlite_db().await;
    let (project_id, _repo_id, agent_id) = seed_project_repo_agent(&db).await;
    let now = now_rfc3339();

    let conversation = ConversationRepo::create(
        &db,
        CreateConversation {
            id: new_uuid_v4(),
            project_id: project_id.clone(),
            agent_id: Some(agent_id.clone()),
            title: "Planning Chat".to_owned(),
            status: ConversationStatus::Active,
            system_prompt: Some("You are a PM".to_owned()),
            message_count: 0,
            last_message_at: None,
            agent_session_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("conversation creates");

    let listed = ConversationRepo::list_by_project(
        &db,
        ConversationListQuery {
            project_id: project_id.clone(),
            status: Some(ConversationStatus::Active),
            page: page(20),
        },
    )
    .await
    .expect("conversation lists");
    assert_eq!(listed.items.len(), 1);
    assert_eq!(listed.items[0].id, conversation.id);

    let sequence_1 = ConversationMessageRepo::next_sequence(&db, &conversation.id)
        .await
        .expect("seq 1");
    assert_eq!(sequence_1, 1);

    let message_1 = ConversationMessageRepo::create(
        &db,
        CreateConversationMessage {
            id: new_uuid_v4(),
            conversation_id: conversation.id.clone(),
            role: ConversationMessageRole::User,
            content: "hello".to_owned(),
            status: ConversationMessageStatus::Complete,
            model: None,
            token_usage_json: None,
            duration_ms: None,
            error: None,
            sequence: sequence_1,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("first message creates");

    let sequence_2 = ConversationMessageRepo::next_sequence(&db, &conversation.id)
        .await
        .expect("seq 2");
    assert_eq!(sequence_2, 2);

    let _message_2 = ConversationMessageRepo::create(
        &db,
        CreateConversationMessage {
            id: new_uuid_v4(),
            conversation_id: conversation.id.clone(),
            role: ConversationMessageRole::Assistant,
            content: "hi".to_owned(),
            status: ConversationMessageStatus::Streaming,
            model: Some("gpt-5".to_owned()),
            token_usage_json: None,
            duration_ms: None,
            error: None,
            sequence: sequence_2,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("second message creates");

    let page_result = ConversationMessageRepo::list_by_conversation(
        &db,
        ConversationMessageListQuery {
            conversation_id: conversation.id.clone(),
            before_sequence: None,
            page: PageRequest {
                cursor: None,
                limit: 50,
                include_total: true,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        },
    )
    .await
    .expect("messages list");
    assert_eq!(page_result.items.len(), 2);
    assert_eq!(page_result.items[0].sequence, 1);
    assert_eq!(page_result.items[1].sequence, 2);

    let active = ConversationMessageRepo::get_active_streaming_message(&db, &conversation.id)
        .await
        .expect("get active")
        .expect("active exists");
    assert_eq!(active.status, ConversationMessageStatus::Streaming);
    assert_eq!(active.sequence, 2);

    let updated = ConversationRepo::update(
        &db,
        UpdateConversation {
            id: conversation.id.clone(),
            expected_version: conversation.version,
            agent_id: None,
            title: Some("Renamed".to_owned()),
            status: Some(ConversationStatus::Archived),
            system_prompt: Some(Some("updated prompt".to_owned())),
            message_count: Some(2),
            last_message_at: Some(Some(now.clone())),
            agent_session_id: Some(Some("sess-1".to_owned())),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("conversation updates");
    assert_eq!(updated.status, ConversationStatus::Archived);
    assert_eq!(updated.title, "Renamed");

    let updated_message = ConversationMessageRepo::update(
        &db,
        crate::UpdateConversationMessage {
            id: message_1.id.clone(),
            content: Some("hello updated".to_owned()),
            status: Some(ConversationMessageStatus::Complete),
            model: None,
            token_usage_json: None,
            duration_ms: None,
            error: None,
            updated_at: now,
        },
    )
    .await
    .expect("message updates");
    assert_eq!(updated_message.content, "hello updated");
}

// ── User / RefreshToken tests ──────────────────────────────────────────────

async fn seed_user(db: &SqliteDb) -> String {
    let now = now_rfc3339();
    let id = new_uuid_v4();
    let user = User {
        id: id.clone(),
        email: format!("user-{}@example.com", id),
        password_hash: "hash".to_owned(),
        display_name: None,
        is_admin: false,
        created_at: now.clone(),
        updated_at: now,
    };
    UserRepo::create_user(db, &user).await.expect("seed user");
    id
}

#[tokio::test]
async fn user_crud() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let user = User {
        id: new_uuid_v4(),
        email: "crud@example.com".to_owned(),
        password_hash: "hash".to_owned(),
        display_name: Some("Test User".to_owned()),
        is_admin: false,
        created_at: now.clone(),
        updated_at: now,
    };

    UserRepo::create_user(&db, &user)
        .await
        .expect("creates user");

    let by_id = UserRepo::get_user_by_id(&db, &user.id)
        .await
        .expect("no error")
        .expect("user exists");
    assert_eq!(by_id.email, user.email);
    assert_eq!(by_id.display_name.as_deref(), Some("Test User"));

    let by_email = UserRepo::get_user_by_email(&db, &user.email)
        .await
        .expect("no error")
        .expect("user found by email");
    assert_eq!(by_email.id, user.id);

    let deleted = UserRepo::delete_user(&db, &user.id)
        .await
        .expect("no error");
    assert!(deleted);

    let gone = UserRepo::get_user_by_id(&db, &user.id)
        .await
        .expect("no error");
    assert!(gone.is_none());
}

#[tokio::test]
async fn user_email_uniqueness() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let make_user = |id: String| User {
        id,
        email: "dup@example.com".to_owned(),
        password_hash: "hash".to_owned(),
        display_name: None,
        is_admin: false,
        created_at: now.clone(),
        updated_at: now.clone(),
    };

    UserRepo::create_user(&db, &make_user(new_uuid_v4()))
        .await
        .expect("first user creates");
    let err = UserRepo::create_user(&db, &make_user(new_uuid_v4())).await;
    assert!(err.is_err(), "duplicate email must fail");
}

#[tokio::test]
async fn refresh_token_lifecycle() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let user_id = seed_user(&db).await;

    let token = RefreshToken {
        id: new_uuid_v4(),
        user_id: user_id.clone(),
        token_hash: "hash-abc".to_owned(),
        family_id: "family-1".to_owned(),
        expires_at: "2099-01-01T00:00:00Z".to_owned(),
        created_at: now,
    };
    RefreshTokenRepo::create_refresh_token(&db, &token)
        .await
        .expect("creates token");

    let found = RefreshTokenRepo::delete_refresh_token_by_hash(&db, "hash-abc")
        .await
        .expect("no error")
        .expect("token returned on first delete");
    assert_eq!(found.user_id, user_id);

    let not_found = RefreshTokenRepo::delete_refresh_token_by_hash(&db, "hash-abc")
        .await
        .expect("no error");
    assert!(not_found.is_none(), "token must not exist after deletion");
}

#[tokio::test]
async fn refresh_token_concurrent_single_winner() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let user_id = seed_user(&db).await;

    let token = RefreshToken {
        id: new_uuid_v4(),
        user_id,
        token_hash: "race-hash".to_owned(),
        family_id: "family-race".to_owned(),
        expires_at: "2099-01-01T00:00:00Z".to_owned(),
        created_at: now,
    };
    RefreshTokenRepo::create_refresh_token(&db, &token)
        .await
        .expect("creates token");

    // Two concurrent DELETE RETURNING on the same hash: exactly one wins.
    let (r1, r2) = tokio::join!(
        RefreshTokenRepo::delete_refresh_token_by_hash(&db, "race-hash"),
        RefreshTokenRepo::delete_refresh_token_by_hash(&db, "race-hash"),
    );
    let r1 = r1.expect("no error on r1");
    let r2 = r2.expect("no error on r2");

    let winners = [r1.is_some(), r2.is_some()].iter().filter(|&&b| b).count();
    assert_eq!(
        winners, 1,
        "exactly one concurrent caller must win the token"
    );
}

#[tokio::test]
async fn refresh_token_family_revocation() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let user_id = seed_user(&db).await;

    for i in 0..3 {
        RefreshTokenRepo::create_refresh_token(
            &db,
            &RefreshToken {
                id: new_uuid_v4(),
                user_id: user_id.clone(),
                token_hash: format!("fam-hash-{i}"),
                family_id: "family-revoke".to_owned(),
                expires_at: "2099-01-01T00:00:00Z".to_owned(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("creates token");
    }
    RefreshTokenRepo::create_refresh_token(
        &db,
        &RefreshToken {
            id: new_uuid_v4(),
            user_id: user_id.clone(),
            token_hash: "other-fam-hash".to_owned(),
            family_id: "family-other".to_owned(),
            expires_at: "2099-01-01T00:00:00Z".to_owned(),
            created_at: now,
        },
    )
    .await
    .expect("creates other token");

    let deleted = RefreshTokenRepo::delete_refresh_tokens_by_family(&db, "family-revoke")
        .await
        .expect("no error");
    assert_eq!(deleted, 3);

    let remaining = RefreshTokenRepo::get_refresh_tokens_by_user(&db, &user_id)
        .await
        .expect("no error");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].family_id, "family-other");
}

#[tokio::test]
async fn refresh_token_expired_cleanup() {
    let db = sqlite_db().await;
    let now = now_rfc3339();
    let user_id = seed_user(&db).await;

    RefreshTokenRepo::create_refresh_token(
        &db,
        &RefreshToken {
            id: new_uuid_v4(),
            user_id: user_id.clone(),
            token_hash: "expired-hash".to_owned(),
            family_id: "family-exp".to_owned(),
            expires_at: "2020-01-01T00:00:00Z".to_owned(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("creates expired token");

    RefreshTokenRepo::create_refresh_token(
        &db,
        &RefreshToken {
            id: new_uuid_v4(),
            user_id: user_id.clone(),
            token_hash: "valid-hash".to_owned(),
            family_id: "family-valid".to_owned(),
            expires_at: "2099-01-01T00:00:00Z".to_owned(),
            created_at: now,
        },
    )
    .await
    .expect("creates valid token");

    let cleaned = RefreshTokenRepo::delete_expired_refresh_tokens(&db)
        .await
        .expect("no error");
    assert_eq!(cleaned, 1);

    let remaining = RefreshTokenRepo::get_refresh_tokens_by_user(&db, &user_id)
        .await
        .expect("no error");
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].token_hash, "valid-hash");
}
