use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, run_migrations_from, ActorRef,
    AgentRepo, AgentStatus, CreateAgentIdentity, CreateAgentProfile, CreateExecution, CreateRepo,
    CreateWorkspace, DbError, ExecutionPurpose, ExecutionRepo, ExecutionStatus, HarnessSessionRepo,
    HarnessSessionStatus, RepoRepo, SqliteDb, UpdateExecution, UpdateHarnessSession, WorkMode,
    WorkspaceRepo, WorkspaceStatus,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

async fn database() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    SqliteDb::new(pool)
}

async fn seed_agent(db: &SqliteDb, agent_id: &str, executor_type: &str) -> String {
    let now = now_rfc3339();
    let profile_id = new_uuid_v4();
    AgentRepo::create_identity_with_profile(
        db,
        CreateAgentIdentity {
            id: agent_id.to_owned(),
            name: format!("Agent {agent_id}"),
            description: None,
            max_concurrent_tasks: 4,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: profile_id.clone(),
            identity_id: agent_id.to_owned(),
            backend_kind: "cli".to_owned(),
            executor_type: executor_type.to_owned(),
            provider: Some("test".to_owned()),
            model: Some("test-model".to_owned()),
            reasoning_effort: None,
            permission_policy: Some("plan".to_owned()),
            prompt_template: None,
            capabilities_json: r#"["read"]"#.to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: r#"{"model":"test-model"}"#.to_owned(),
            credential_ref: None,
            daemon_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("agent creates");
    profile_id
}

async fn seed_task(db: &SqliteDb, suffix: &str) -> String {
    let now = now_rfc3339();
    let project_id = format!("pr2-project-{suffix}");
    let task_id = format!("pr2-task-{suffix}");
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES (?, ?, '{}', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind(format!("PR2 {suffix}"))
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("project creates");
    sqlx::query(
        "INSERT INTO task (id, project_id, title, task_type, status, created_at, updated_at)
         VALUES (?, ?, 'PR2 task', 'implementation', 'in_progress', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("task creates");
    task_id
}

async fn seed_workspaces(db: &SqliteDb, task_id: &str) -> (String, String) {
    let now = now_rfc3339();
    let project_id: String = sqlx::query_scalar("SELECT project_id FROM task WHERE id = ?")
        .bind(task_id)
        .fetch_one(db.pool())
        .await
        .expect("task project loads");
    let repo_id = format!("pr2-repo-{task_id}");
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id,
            name: "PR2 repo".to_owned(),
            remote_url: "https://example.invalid/pr2.git".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo creates");
    sqlx::query("UPDATE task SET repo_id = ? WHERE id = ?")
        .bind(&repo_id)
        .bind(task_id)
        .execute(db.pool())
        .await
        .expect("task repo binds");

    let first = format!("pr2-workspace-{task_id}-one");
    let second = format!("pr2-workspace-{task_id}-two");
    for (id, branch) in [(&first, "pr2-one"), (&second, "pr2-two")] {
        WorkspaceRepo::create(
            db,
            CreateWorkspace {
                id: id.clone(),
                task_id: task_id.to_owned(),
                repo_id: repo_id.clone(),
                worktree_path: format!("/tmp/{id}"),
                branch: branch.to_owned(),
                status: WorkspaceStatus::Ready,
                before_sha: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("workspace creates");
    }
    (first, second)
}

fn execution_input(
    id: &str,
    task_id: &str,
    agent_id: &str,
    profile_id: &str,
    executor_type: &str,
    purpose: ExecutionPurpose,
    workspace_id: Option<&str>,
    harness_session_id: Option<String>,
    agent_session_id: Option<String>,
) -> CreateExecution {
    CreateExecution {
        id: id.to_owned(),
        task_id: task_id.to_owned(),
        agent_id: Some(agent_id.to_owned()),
        actor_ref: Some(ActorRef::Agent(agent_id.to_owned())),
        role: "coder".to_owned(),
        purpose: Some(purpose),
        status: ExecutionStatus::Running,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        parent_execution_id: None,
        agent_session_id,
        harness_session_id,
        agent_message_id: None,
        last_activity_at: None,
        summary: None,
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: Some(
            serde_json::json!({
                "agent_id": agent_id,
                "profile_id": profile_id,
                "executor_type": executor_type,
                "capabilities": ["read"],
                "config": {"model": "test-model"}
            })
            .to_string(),
        ),
        workspace_id: workspace_id.map(str::to_owned),
        created_at: now_rfc3339(),
        updated_at: now_rfc3339(),
    }
}

fn result_update(execution_id: &str, external_session_id: Option<&str>) -> UpdateExecution {
    UpdateExecution {
        id: execution_id.to_owned(),
        status: None,
        stop_reason: None,
        stopped_by: None,
        resume_policy: None,
        stopped_at: None,
        agent_session_id: Some(external_session_id.map(str::to_owned)),
        agent_message_id: None,
        last_activity_at: None,
        summary: None,
        logs_path: None,
        before_sha: None,
        after_sha: None,
        error: None,
        executor_config_snapshot_json: None,
        updated_at: now_rfc3339(),
    }
}

#[tokio::test]
async fn fresh_agent_execution_has_one_pending_session_and_atomic_result_projection() {
    let db = database().await;
    let profile_id = seed_agent(&db, "pr2-agent-a", "codex").await;
    let task_id = seed_task(&db, "fresh").await;
    let execution_id = "pr2-execution-fresh";
    let execution = ExecutionRepo::create(
        &db,
        execution_input(
            execution_id,
            &task_id,
            "pr2-agent-a",
            &profile_id,
            "codex",
            ExecutionPurpose::Implement,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("execution creates");
    let session_id = execution
        .harness_session_id
        .clone()
        .expect("fresh Agent execution materializes a session");
    let pending = HarnessSessionRepo::get_by_id(&db, &session_id)
        .await
        .expect("session loads")
        .expect("session exists");
    assert_eq!(pending.status, HarnessSessionStatus::Pending);
    assert_eq!(pending.agent_id, "pr2-agent-a");
    assert_eq!(pending.harness_kind, "codex");
    assert_eq!(pending.profile_id.as_deref(), Some(profile_id.as_str()));
    assert!(pending.profile_snapshot_json.contains("test-model"));
    assert_eq!(pending.capabilities_snapshot_json, r#"["read"]"#);

    let activated = ExecutionRepo::update(&db, result_update(execution_id, Some("thread-1")))
        .await
        .expect("executor result persists");
    assert_eq!(
        activated.harness_session_id.as_deref(),
        Some(session_id.as_str())
    );
    assert_eq!(activated.agent_session_id.as_deref(), Some("thread-1"));
    let active = HarnessSessionRepo::get_by_id(&db, &session_id)
        .await
        .expect("session loads")
        .expect("session exists");
    assert_eq!(active.status, HarnessSessionStatus::Active);
    assert_eq!(active.external_session_id.as_deref(), Some("thread-1"));

    let mut failed_update = result_update(execution_id, None);
    failed_update.status = Some(ExecutionStatus::Failed);
    ExecutionRepo::update(&db, failed_update)
        .await
        .expect("failed Execution persists");
    let reusable_after_failure = HarnessSessionRepo::get_by_id(&db, &session_id)
        .await
        .expect("session reloads after failure")
        .expect("session exists after failure");
    assert_eq!(reusable_after_failure.status, HarnessSessionStatus::Active);

    let repeated = ExecutionRepo::update(&db, result_update(execution_id, Some("thread-1")))
        .await
        .expect("retrying the same result is idempotent");
    assert_eq!(
        repeated.harness_session_id.as_deref(),
        Some(session_id.as_str())
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM harness_session WHERE agent_id = 'pr2-agent-a' AND external_session_id = 'thread-1'",
    )
    .fetch_one(db.pool())
    .await
    .expect("session count loads");
    assert_eq!(count, 1);
    let divergent = ExecutionRepo::update(&db, result_update(execution_id, Some("thread-2")))
        .await
        .expect_err("one Execution cannot change its external session identity");
    assert!(matches!(divergent, DbError::Check(_)));
}

#[tokio::test]
async fn human_execution_has_real_actor_and_no_agent_or_harness_session() {
    let db = database().await;
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES ('pr2-human', 'pr2-human@example.test', 'test', 'Human', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("human creates");
    let task_id = seed_task(&db, "human").await;
    let execution = ExecutionRepo::create(
        &db,
        CreateExecution {
            id: "pr2-execution-human".to_owned(),
            task_id,
            agent_id: None,
            actor_ref: Some(ActorRef::Human("pr2-human".to_owned())),
            role: "reviewer".to_owned(),
            purpose: Some(ExecutionPurpose::Review),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            harness_session_id: None,
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
    .expect("human execution creates");
    assert_eq!(
        execution.actor_ref(),
        Some(ActorRef::Human("pr2-human".to_owned()))
    );
    assert_eq!(execution.agent_id, None);
    assert_eq!(execution.harness_session_id, None);
    assert!(matches!(
        ExecutionRepo::create(
            &db,
            CreateExecution {
                id: "pr2-execution-fake-human".to_owned(),
                task_id: execution.task_id.clone(),
                agent_id: None,
                actor_ref: Some(ActorRef::Human("human".to_owned())),
                role: "reviewer".to_owned(),
                purpose: Some(ExecutionPurpose::Review),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                harness_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
        )
        .await,
        Err(DbError::Sqlx(_)) | Err(DbError::Check(_))
    ));
}

#[tokio::test]
async fn purpose_is_independent_of_role_and_actor_change_cannot_reuse_session() {
    let db = database().await;
    let profile_a = seed_agent(&db, "pr2-agent-a", "codex").await;
    let profile_b = seed_agent(&db, "pr2-agent-b", "codex").await;
    let task_id = seed_task(&db, "purpose").await;
    let first = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-implement",
            &task_id,
            "pr2-agent-a",
            &profile_a,
            "codex",
            ExecutionPurpose::Implement,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("first execution creates");
    let second = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-investigate",
            &task_id,
            "pr2-agent-a",
            &profile_a,
            "codex",
            ExecutionPurpose::Investigate,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("second execution creates");
    assert_eq!(first.role, second.role);
    assert_eq!(first.purpose, Some(ExecutionPurpose::Implement));
    assert_eq!(second.purpose, Some(ExecutionPurpose::Investigate));

    let first = ExecutionRepo::record_harness_session_result(
        &db,
        &first.id,
        "shared-role-is-not-authority",
        &now_rfc3339(),
    )
    .await
    .expect("first session activates");
    let second = ExecutionRepo::record_harness_session_result(
        &db,
        &second.id,
        "newer-unrelated-session",
        &now_rfc3339(),
    )
    .await
    .expect("unrelated session activates");
    let first_session = first.harness_session_id.clone().expect("first session");
    assert_ne!(first.harness_session_id, second.harness_session_id);
    let same_actor_child = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-same-actor-child",
            &task_id,
            "pr2-agent-a",
            &profile_a,
            "codex",
            ExecutionPurpose::General,
            None,
            Some(first_session.clone()),
            Some("shared-role-is-not-authority".to_owned()),
        ),
    )
    .await
    .expect("explicit same-Actor continuity creates");
    assert_eq!(
        same_actor_child.harness_session_id.as_deref(),
        Some(first_session.as_str())
    );

    let inferred = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-inferred-session",
            &task_id,
            "pr2-agent-a",
            &profile_a,
            "codex",
            ExecutionPurpose::General,
            None,
            None,
            Some("shared-role-is-not-authority".to_owned()),
        ),
    )
    .await
    .expect_err("legacy external identity cannot select a new session");
    assert!(matches!(inferred, DbError::Check(_)));

    let other_actor = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-other-actor",
            &task_id,
            "pr2-agent-b",
            &profile_b,
            "codex",
            ExecutionPurpose::Implement,
            None,
            Some(first_session),
            None,
        ),
    )
    .await
    .expect_err("different Actor cannot inherit a session");
    assert!(matches!(other_actor, DbError::Check(_)));
}

#[tokio::test]
async fn execution_purpose_does_not_alias_permission_policy() {
    let db = database().await;
    let profile_id = seed_agent(&db, "pr2-agent-permission", "codex").await;
    sqlx::query("UPDATE agent_profile SET permission_policy = 'auto' WHERE id = ?")
        .bind(&profile_id)
        .execute(db.pool())
        .await
        .expect("permission policy updates");
    let task_id = seed_task(&db, "purpose-permission").await;
    let execution = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-plan-purpose",
            &task_id,
            "pr2-agent-permission",
            &profile_id,
            "codex",
            ExecutionPurpose::Plan,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("plan execution creates");
    assert_eq!(execution.purpose, Some(ExecutionPurpose::Plan));
    let permission_policy: String =
        sqlx::query_scalar("SELECT permission_policy FROM agent_profile WHERE id = ?")
            .bind(&profile_id)
            .fetch_one(db.pool())
            .await
            .expect("permission policy loads");
    assert_eq!(permission_policy, "auto");
}

#[tokio::test]
async fn workspace_scoped_session_is_not_reused_in_another_workspace() {
    let db = database().await;
    let profile_id = seed_agent(&db, "pr2-agent-workspace", "codex").await;
    let task_id = seed_task(&db, "workspace").await;
    let (workspace_one, workspace_two) = seed_workspaces(&db, &task_id).await;
    let parent = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-workspace-parent",
            &task_id,
            "pr2-agent-workspace",
            &profile_id,
            "codex",
            ExecutionPurpose::Implement,
            Some(&workspace_one),
            None,
            None,
        ),
    )
    .await
    .expect("workspace parent creates");
    let parent = ExecutionRepo::record_harness_session_result(
        &db,
        &parent.id,
        "workspace-thread",
        &now_rfc3339(),
    )
    .await
    .expect("workspace session activates");
    let session_id = parent.harness_session_id.clone().expect("session");
    let child = execution_input(
        "pr2-execution-workspace-child",
        &task_id,
        "pr2-agent-workspace",
        &profile_id,
        "codex",
        ExecutionPurpose::Implement,
        Some(&workspace_two),
        Some(session_id),
        Some("workspace-thread".to_owned()),
    );
    let error = ExecutionRepo::create(&db, child)
        .await
        .expect_err("workspace mismatch must fail closed");
    assert!(matches!(error, DbError::Check(_)));
}

#[tokio::test]
async fn external_identity_is_scoped_by_agent_and_harness() {
    let db = database().await;
    let profile_a = seed_agent(&db, "pr2-agent-collision-a", "codex").await;
    let profile_b = seed_agent(&db, "pr2-agent-collision-b", "codex").await;
    let task_id = seed_task(&db, "collision").await;
    let a = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-collision-a",
            &task_id,
            "pr2-agent-collision-a",
            &profile_a,
            "codex",
            ExecutionPurpose::Implement,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("agent A execution creates");
    let a = ExecutionRepo::record_harness_session_result(&db, &a.id, "abc", &now_rfc3339())
        .await
        .expect("agent A session activates");
    let b = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-collision-b",
            &task_id,
            "pr2-agent-collision-b",
            &profile_b,
            "codex",
            ExecutionPurpose::Implement,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("agent B execution creates");
    let b = ExecutionRepo::record_harness_session_result(&db, &b.id, "abc", &now_rfc3339())
        .await
        .expect("agent B session activates");
    assert_ne!(a.harness_session_id, b.harness_session_id);

    let other_harness = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-collision-harness",
            &task_id,
            "pr2-agent-collision-a",
            &profile_a,
            "cursor",
            ExecutionPurpose::Implement,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("different harness execution creates");
    let other_harness =
        ExecutionRepo::record_harness_session_result(&db, &other_harness.id, "abc", &now_rfc3339())
            .await
            .expect("different harness session activates");
    assert_ne!(a.harness_session_id, other_harness.harness_session_id);
}

#[tokio::test]
async fn session_identity_and_execution_history_are_immutable_and_snapshotted() {
    let db = database().await;
    let profile_id = seed_agent(&db, "pr2-agent-immutable", "codex").await;
    seed_agent(&db, "pr2-agent-immutable-other", "cursor").await;
    let task_id = seed_task(&db, "immutable").await;
    let execution = ExecutionRepo::create(
        &db,
        execution_input(
            "pr2-execution-immutable",
            &task_id,
            "pr2-agent-immutable",
            &profile_id,
            "codex",
            ExecutionPurpose::Implement,
            None,
            None,
            None,
        ),
    )
    .await
    .expect("execution creates");
    let session_id = execution.harness_session_id.clone().expect("session");
    let actor_change = sqlx::query(
        "UPDATE execution SET actor_id = 'pr2-agent-immutable-other' WHERE id = 'pr2-execution-immutable'",
    )
    .execute(db.pool())
    .await
    .expect_err("Execution Actor cannot be rewritten");
    assert!(actor_change.to_string().contains("immutable"));
    let purpose_change = sqlx::query(
        "UPDATE execution SET purpose = 'investigate' WHERE id = 'pr2-execution-immutable'",
    )
    .execute(db.pool())
    .await
    .expect_err("Execution Purpose cannot be rewritten");
    assert!(purpose_change.to_string().contains("immutable"));
    let session_agent_change =
        sqlx::query("UPDATE harness_session SET agent_id = 'pr2-agent-other' WHERE id = ?")
            .bind(&session_id)
            .execute(db.pool())
            .await
            .expect_err("HarnessSession Agent cannot be reassigned");
    assert!(session_agent_change.to_string().contains("immutable"));
    let session_harness_change =
        sqlx::query("UPDATE harness_session SET harness_kind = 'cursor' WHERE id = ?")
            .bind(&session_id)
            .execute(db.pool())
            .await
            .expect_err("HarnessSession harness identity cannot be reassigned");
    assert!(session_harness_change.to_string().contains("immutable"));

    let snapshot_before: String =
        sqlx::query_scalar("SELECT profile_snapshot_json FROM harness_session WHERE id = ?")
            .bind(&session_id)
            .fetch_one(db.pool())
            .await
            .expect("session snapshot loads");
    sqlx::query("UPDATE agent_profile SET config_json = '{\"model\":\"changed\"}' WHERE id = ?")
        .bind(&profile_id)
        .execute(db.pool())
        .await
        .expect("profile changes");
    let snapshot_after: String =
        sqlx::query_scalar("SELECT profile_snapshot_json FROM harness_session WHERE id = ?")
            .bind(&session_id)
            .fetch_one(db.pool())
            .await
            .expect("session snapshot reloads");
    assert_eq!(snapshot_before, snapshot_after);

    let ended = HarnessSessionRepo::update(
        &db,
        UpdateHarnessSession {
            id: session_id,
            external_session_id: None,
            status: Some(HarnessSessionStatus::Ended),
            last_activity_at: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("session ends explicitly");
    assert_eq!(ended.status, HarnessSessionStatus::Ended);
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("forge-pr2-{label}-{nanos}"))
}

fn copy_migrations_up_to(max_version: i64, destination: &Path) {
    fs::create_dir_all(destination).expect("migration dir creates");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in fs::read_dir(source).expect("migration dir reads") {
        let entry = entry.expect("migration entry reads");
        let file_name = entry.file_name();
        let name = file_name.to_string_lossy();
        let Some(version) = name
            .strip_prefix('V')
            .and_then(|value| value.split_once("__"))
            .and_then(|(value, _)| value.parse::<i64>().ok())
        else {
            continue;
        };
        if version <= max_version {
            fs::copy(entry.path(), destination.join(file_name)).expect("migration copies");
        }
    }
}

async fn seed_legacy_execution(
    db: &SqliteDb,
    id: &str,
    task_id: &str,
    agent_id: Option<&str>,
    role: &str,
    external_session_id: Option<&str>,
    snapshot: Option<&str>,
    created_at: &str,
) {
    sqlx::query(
        "INSERT INTO execution
            (id, task_id, agent_id, role, status, agent_session_id,
             executor_config_snapshot_json, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'completed', ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(task_id)
    .bind(agent_id)
    .bind(role)
    .bind(external_session_id)
    .bind(snapshot)
    .bind(created_at)
    .bind(created_at)
    .execute(db.pool())
    .await
    .expect("legacy execution inserts");
}

#[tokio::test]
async fn historical_session_migration_groups_only_coherent_identity() {
    let migration_dir = unique_temp_dir("migration");
    copy_migrations_up_to(88, &migration_dir);
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("legacy migrations apply");
    let db = SqliteDb::new(pool.clone());
    let profile_a = seed_agent(&db, "pr2-history-a", "codex").await;
    let profile_b = seed_agent(&db, "pr2-history-b", "codex").await;
    let task_id = seed_task(&db, "history").await;
    let snapshot_a = serde_json::json!({
        "executor_type": "codex",
        "profile_id": profile_a,
        "capabilities": ["read"]
    })
    .to_string();
    let snapshot_b = serde_json::json!({
        "executor_type": "codex",
        "profile_id": profile_b,
        "capabilities": ["read"]
    })
    .to_string();
    let snapshot_cursor = serde_json::json!({
        "executor_type": "cursor",
        "profile_id": profile_a
    })
    .to_string();
    let snapshot_ambiguous = serde_json::json!({
        "executor_type": "codex",
        "profile_id": "history-profile-one"
    })
    .to_string();
    let snapshot_ambiguous_two = serde_json::json!({
        "executor_type": "codex",
        "profile_id": "history-profile-two"
    })
    .to_string();
    seed_legacy_execution(
        &db,
        "pr2-history-a-one",
        &task_id,
        Some("pr2-history-a"),
        "coder",
        Some("same-thread"),
        Some(&snapshot_a),
        "2026-01-01T00:00:00Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-a-two",
        &task_id,
        Some("pr2-history-a"),
        "reviewer",
        Some("same-thread"),
        Some(&snapshot_a),
        "2026-01-01T00:01:00Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-planner",
        &task_id,
        Some("pr2-history-a"),
        "planner",
        Some("planner-thread"),
        Some(&snapshot_a),
        "2026-01-01T00:01:30Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-unknown-role",
        &task_id,
        Some("pr2-history-a"),
        "custom-role",
        Some("unknown-role-thread"),
        Some(&snapshot_a),
        "2026-01-01T00:01:45Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-b",
        &task_id,
        Some("pr2-history-b"),
        "coder",
        Some("same-thread"),
        Some(&snapshot_b),
        "2026-01-01T00:02:00Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-cursor",
        &task_id,
        Some("pr2-history-a"),
        "coder",
        Some("same-thread"),
        Some(&snapshot_cursor),
        "2026-01-01T00:03:00Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-ambiguous-one",
        &task_id,
        Some("pr2-history-a"),
        "coder",
        Some("ambiguous-thread"),
        Some(&snapshot_ambiguous),
        "2026-01-01T00:04:00Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-ambiguous-two",
        &task_id,
        Some("pr2-history-a"),
        "coder",
        Some("ambiguous-thread"),
        Some(&snapshot_ambiguous_two),
        "2026-01-01T00:05:00Z",
    )
    .await;
    seed_legacy_execution(
        &db,
        "pr2-history-agentless",
        &task_id,
        None,
        "system",
        Some("unknown-thread"),
        None,
        "2026-01-01T00:06:00Z",
    )
    .await;

    run_migrations(&pool).await.expect("PR2 migration applies");
    let first = ExecutionRepo::get_by_id(&db, "pr2-history-a-one")
        .await
        .expect("first execution loads")
        .expect("first execution exists");
    let second = ExecutionRepo::get_by_id(&db, "pr2-history-a-two")
        .await
        .expect("second execution loads")
        .expect("second execution exists");
    let planner = ExecutionRepo::get_by_id(&db, "pr2-history-planner")
        .await
        .expect("planner execution loads")
        .expect("planner execution exists");
    let unknown_role = ExecutionRepo::get_by_id(&db, "pr2-history-unknown-role")
        .await
        .expect("unknown-role execution loads")
        .expect("unknown-role execution exists");
    let other_agent = ExecutionRepo::get_by_id(&db, "pr2-history-b")
        .await
        .expect("other agent execution loads")
        .expect("other agent execution exists");
    let other_harness = ExecutionRepo::get_by_id(&db, "pr2-history-cursor")
        .await
        .expect("other harness execution loads")
        .expect("other harness execution exists");
    let ambiguous = ExecutionRepo::get_by_id(&db, "pr2-history-ambiguous-one")
        .await
        .expect("ambiguous execution loads")
        .expect("ambiguous execution exists");
    let agentless = ExecutionRepo::get_by_id(&db, "pr2-history-agentless")
        .await
        .expect("agentless execution loads")
        .expect("agentless execution exists");
    assert_eq!(
        first.actor_ref(),
        Some(ActorRef::Agent("pr2-history-a".to_owned()))
    );
    assert_eq!(first.purpose, Some(ExecutionPurpose::Implement));
    assert_eq!(second.purpose, Some(ExecutionPurpose::Review));
    assert_eq!(planner.purpose, Some(ExecutionPurpose::Plan));
    assert_eq!(unknown_role.purpose, Some(ExecutionPurpose::General));
    assert_eq!(first.harness_session_id, second.harness_session_id);
    assert_ne!(first.harness_session_id, other_agent.harness_session_id);
    assert_ne!(first.harness_session_id, other_harness.harness_session_id);
    assert_eq!(ambiguous.harness_session_id, None);
    assert_eq!(agentless.actor_ref(), None);
    assert_eq!(agentless.purpose, Some(ExecutionPurpose::General));
    let issue_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_session_migration_issue WHERE issue_kind = 'historical_session_ambiguous'",
    )
    .fetch_one(db.pool())
    .await
    .expect("ambiguity issues load");
    assert_eq!(issue_count, 2);
    let actor_issue_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM execution_session_migration_issue WHERE issue_kind = 'historical_actor_unresolved' AND execution_id = 'pr2-history-agentless'",
    )
    .fetch_one(db.pool())
    .await
    .expect("actor issue loads");
    assert_eq!(actor_issue_count, 1);
    let _ = fs::remove_dir_all(migration_dir);
}
