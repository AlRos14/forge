use db::{
    create_sqlite_pool, run_migrations, run_migrations_from, AgentRepo, AgentStatus, CreateAgent,
    CreateDomainEvent, CreateProject, CreateTask, CreateTaskIntegrationOperation, DomainEventRepo,
    ProjectRepo, SqliteDb, TaskIntegrationOperationKind, TaskIntegrationOperationRepo,
    TaskIntegrationOperationStatus, TaskRepo, User, UserRepo,
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

fn unique_temp_path(name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("forge-{name}-{}-{nanos}", std::process::id()))
}

#[tokio::test]
async fn file_backed_migrations_apply_and_task_operation_claim_is_atomic_across_pools() {
    let db_path = unique_temp_path("migtest").with_extension("db");
    let _ = std::fs::remove_file(&db_path);
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations(&pool).await.expect("migrations");

    let db = SqliteDb::new(pool.clone());
    let now = db::now_rfc3339();
    let project_id = db::new_uuid_v4();
    let task_id = db::new_uuid_v4();
    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Durable Task operation claim".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project");
    TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: None,
            parent_task_id: None,
            title: "integration target".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Task");

    // Separate SQLite pools stand in for independent service processes. Both
    // submit their claim concurrently; the partial unique index admits one.
    let second_pool = create_sqlite_pool(&url).await.expect("second pool");
    let second_db = SqliteDb::new(second_pool.clone());
    let first_claim = CreateTaskIntegrationOperation {
        id: db::new_uuid_v4(),
        task_id: task_id.clone(),
        kind: TaskIntegrationOperationKind::WorkUnitIntegration,
        owner_id: "integration-attempt-a".to_owned(),
        gate_evaluation_id: None,
        remote_waiting: false,
        parent_operation_id: None,
        created_at: db::now_rfc3339(),
    };
    let second_claim = CreateTaskIntegrationOperation {
        id: db::new_uuid_v4(),
        task_id: task_id.clone(),
        kind: TaskIntegrationOperationKind::IntegrationWorkspaceCleanup,
        owner_id: "merge-attempt-b".to_owned(),
        gate_evaluation_id: None,
        remote_waiting: false,
        parent_operation_id: None,
        created_at: db::now_rfc3339(),
    };
    let (first, second) = tokio::join!(
        TaskIntegrationOperationRepo::begin(&db, first_claim),
        TaskIntegrationOperationRepo::begin(&second_db, second_claim),
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let rejected = if first.is_err() {
        first.err()
    } else {
        second.err()
    };
    assert!(matches!(
        rejected,
        Some(db::DbError::TaskIntegrationOperationBusy)
    ));
    let active = TaskIntegrationOperationRepo::get_active_for_task(&db, &task_id)
        .await
        .expect("active operation query")
        .expect("one durable active claim");
    assert_eq!(active.status, TaskIntegrationOperationStatus::Running);
}

#[tokio::test]
async fn pr9_migration_maps_legacy_task_states_conservatively_and_audits_ambiguity() {
    let base_dir = unique_temp_path("pr9-v099-migrations");
    let final_dir = unique_temp_path("pr9-v100-migrations");
    fs::create_dir_all(&base_dir).expect("base migration dir");
    fs::create_dir_all(&final_dir).expect("final migration dir");
    copy_migrations_up_to(99, &base_dir);
    copy_migrations_up_to(100, &final_dir);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &base_dir)
        .await
        .expect("V099 baseline");
    let db = SqliteDb::new(pool.clone());
    let now = db::now_rfc3339();
    let project_id = db::new_uuid_v4();
    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "PR9 lifecycle migration".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project");

    let expected = [
        ("backlog", "backlog", "mapped"),
        ("todo", "ready", "mapped"),
        ("ready", "ready", "mapped"),
        ("planning", "active", "mapped"),
        ("in_progress", "active", "mapped"),
        ("working", "active", "mapped"),
        ("review", "blocked", "ambiguous_review"),
        ("merge_failed", "blocked", "merge_failed"),
        ("merging", "blocked", "ambiguous_merge"),
        ("custom_state", "blocked", "unknown_custom_state"),
        ("done", "done", "mapped"),
        ("cancelled", "cancelled", "mapped"),
    ];
    let mut task_ids = Vec::new();
    for (legacy_state, _, _) in expected {
        let task_id = db::new_uuid_v4();
        sqlx::query(
            "INSERT INTO task (id, project_id, title, task_type, status, created_at, updated_at)
             VALUES (?, ?, ?, 'implementation', ?, ?, ?)",
        )
        .bind(&task_id)
        .bind(&project_id)
        .bind(format!("legacy {legacy_state}"))
        .bind(legacy_state)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("legacy Task row inserts before V100");
        task_ids.push((task_id, legacy_state));
    }
    let review_task_id = task_ids
        .iter()
        .find(|(_, status)| *status == "review")
        .expect("review migration fixture exists");
    sqlx::query("UPDATE task SET entry_barrier_json = ? WHERE id = ?")
        .bind("{\"status\":\"running\",\"state\":\"review\"}")
        .bind(&review_task_id.0)
        .execute(&pool)
        .await
        .expect("legacy entry barrier setup");
    let blocked_active_id = db::new_uuid_v4();
    TaskRepo::create(
        &db,
        CreateTask {
            id: blocked_active_id.clone(),
            project_id: project_id.clone(),
            repo_id: None,
            parent_task_id: None,
            title: "active Task with legacy block metadata".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Active,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("active blocked Task");
    sqlx::query("UPDATE task SET error_annotation = ? WHERE id = ?")
        .bind("{\"type\":\"manual_stop\"}")
        .bind(&blocked_active_id)
        .execute(&pool)
        .await
        .expect("legacy active blocker setup");

    run_migrations_from(&pool, &final_dir)
        .await
        .expect("V100 migration");
    for ((task_id, _), (_, mapped, status)) in task_ids.iter().zip(expected) {
        let actual: (String, String, Option<String>) = sqlx::query_as(
            "SELECT l.state, a.mapping_status, t.entry_barrier_json
             FROM task_lifecycle l
             JOIN task_lifecycle_migration_audit a ON a.task_id = l.task_id
             JOIN task t ON t.id = l.task_id
             WHERE l.task_id = ?",
        )
        .bind(task_id)
        .fetch_one(&pool)
        .await
        .expect("mapped lifecycle lookup");
        assert_eq!(actual, (mapped.to_owned(), status.to_owned(), None));
    }
    let audit_details: String = sqlx::query_scalar(
        "SELECT details_json FROM task_lifecycle_migration_audit WHERE task_id = ?",
    )
    .bind(&review_task_id.0)
    .fetch_one(&pool)
    .await
    .expect("legacy uncertainty audit");
    assert!(audit_details.contains("legacy_entry_barrier_json"));
    assert!(audit_details.contains("running"));
    let blocked_mapping: (String, String, String, String) = sqlx::query_as(
        "SELECT l.state, a.mapping_status, a.reason_kind, a.reason_ref
         FROM task_lifecycle l
         JOIN task_lifecycle_migration_audit a ON a.task_id = l.task_id
         WHERE l.task_id = ?",
    )
    .bind(&blocked_active_id)
    .fetch_one(&pool)
    .await
    .expect("active legacy blocker maps fail closed");
    assert_eq!(
        blocked_mapping,
        (
            "blocked".to_owned(),
            "legacy_active_block_metadata".to_owned(),
            "legacy_blocked_task_metadata".to_owned(),
            blocked_active_id,
        )
    );

    let _ = fs::remove_dir_all(base_dir);
    let _ = fs::remove_dir_all(final_dir);
}

#[tokio::test]
async fn pr5_rebuild_preserves_legacy_workspace_as_task_integration_scope() {
    let migration_dir = unique_temp_path("pr5-legacy-migrations");
    fs::create_dir_all(&migration_dir).expect("migration dir");
    copy_migrations_up_to(90, &migration_dir);

    let db_path = unique_temp_path("pr5-workspace-preserve").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("baseline through V090");
    let db = SqliteDb::new(pool.clone());
    let now = "2026-09-28T00:00:00Z";
    let project_id = db::new_uuid_v4();
    let repo_id = db::new_uuid_v4();
    let task_id = db::new_uuid_v4();
    let workspace_id = db::new_uuid_v4();
    db::ProjectRepo::create(
        &db,
        db::CreateProject {
            id: project_id.clone(),
            name: "Legacy Workspace".into(),
            settings: "{}".into(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.into(),
            updated_at: now.into(),
        },
    )
    .await
    .expect("Project");
    db::RepoRepo::create(
        &db,
        db::CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "repo".into(),
            remote_url: "https://example.invalid/repo.git".into(),
            local_path: None,
            work_mode: db::WorkMode::DirectMerge,
            default_branch: "main".into(),
            created_at: now.into(),
            updated_at: now.into(),
        },
    )
    .await
    .expect("Repo");
    db::TaskRepo::create(
        &db,
        db::CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: Some(repo_id.clone()),
            parent_task_id: None,
            title: "legacy task".into(),
            description: None,
            task_type: "implementation".into(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.into(),
            updated_at: now.into(),
        },
    )
    .await
    .expect("Task");
    sqlx::query(
        "INSERT INTO workspace (id, task_id, repo_id, worktree_path, branch, status, before_sha, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'ready', ?, ?, ?)",
    ).bind(&workspace_id).bind(&task_id).bind(&repo_id)
        .bind("/legacy/task/worktree").bind("task/legacy").bind("abc123")
        .bind(now).bind(now).execute(&pool).await.expect("legacy workspace");

    let human_id = db::new_uuid_v4();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES (?, ?, 'test-hash', 'Migration Actor', ?, ?)",
    )
    .bind(&human_id)
    .bind(format!("{}@example.invalid", human_id))
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy provenance Human exists");
    let ambiguous_actor_id = db::new_uuid_v4();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES (?, ?, 'test-hash', 'Ambiguous Migration Actor', ?, ?)",
    )
    .bind(&ambiguous_actor_id)
    .bind(format!("{}@example.invalid", ambiguous_actor_id))
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("ambiguous provenance Human exists");
    AgentRepo::create(
        &SqliteDb::new(pool.clone()),
        CreateAgent {
            id: ambiguous_actor_id.clone(),
            name: "Ambiguous Migration Actor".to_owned(),
            description: None,
            executor_type: "null".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .expect("same ID can exist in independent legacy Actor tables");

    // Apply V091 separately so this fixture represents a real database that
    // has already stored untyped Actor provenance before V092 is installed.
    copy_migrations_up_to(91, &migration_dir);
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("V091 applies");

    let role_id = db::new_uuid_v4();
    sqlx::query(
        "INSERT INTO task_role (id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at)
         VALUES (?, ?, 'executor', NULL, '{}', 1, ?, ?)",
    )
    .bind(&role_id)
    .bind(&task_id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("WorkUnit Role exists");

    let known_actor_work_unit_id = db::new_uuid_v4();
    let unresolved_actor_work_unit_id = db::new_uuid_v4();
    let ambiguous_actor_work_unit_id = db::new_uuid_v4();
    for (work_unit_id, provenance_id) in [
        (&known_actor_work_unit_id, human_id.as_str()),
        (&unresolved_actor_work_unit_id, "missing-legacy-actor"),
        (&ambiguous_actor_work_unit_id, ambiguous_actor_id.as_str()),
    ] {
        sqlx::query(
            "INSERT INTO work_unit
                (id, task_id, title, scope, status, role, requires_integration,
                 provenance_kind, provenance_id, created_by_kind, created_by_id,
                 version, created_at, updated_at)
             VALUES (?, ?, 'legacy provenance', 'migration fixture', 'open', 'executor', 1,
                     'actor', ?, 'human', ?, 1, ?, ?)",
        )
        .bind(work_unit_id)
        .bind(&task_id)
        .bind(provenance_id)
        .bind(&human_id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("V091 WorkUnit with untyped Actor provenance inserts");
    }

    copy_migrations_up_to(92, &migration_dir);
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("V092 applies");
    let row: (String, String, String, String, Option<String>) = sqlx::query_as(
        "SELECT id, worktree_path, branch, status, before_sha FROM workspace WHERE task_id = ?",
    )
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .expect("workspace row remains");
    assert_eq!(
        row,
        (
            workspace_id.clone(),
            "/legacy/task/worktree".into(),
            "task/legacy".into(),
            "ready".into(),
            Some("abc123".into())
        )
    );
    let scope: (String, Option<String>) = sqlx::query_as(
        "SELECT scope_kind, work_unit_id FROM workspace_scope WHERE workspace_id = ?",
    )
    .bind(&workspace_id)
    .fetch_one(&pool)
    .await
    .expect("legacy scope");
    assert_eq!(scope, ("integration".into(), None));
    let canonical = db::WorkspaceRepo::get_integration_by_task_id(&db, &task_id)
        .await
        .expect("legacy get")
        .expect("integration workspace");
    assert_eq!(canonical.id, workspace_id);

    let known = db::WorkUnitRepo::get_by_id(&db, &known_actor_work_unit_id)
        .await
        .expect("known provenance loads")
        .expect("known WorkUnit exists");
    assert_eq!(
        known.provenance,
        Some(db::WorkUnitProvenance::Actor(db::ActorRef::Human(
            human_id.clone()
        )))
    );
    let unresolved = db::WorkUnitRepo::get_by_id(&db, &unresolved_actor_work_unit_id)
        .await
        .expect("unresolved provenance loads")
        .expect("unresolved WorkUnit exists");
    assert_eq!(
        unresolved.provenance,
        Some(db::WorkUnitProvenance::LegacyActor(
            "missing-legacy-actor".to_owned()
        ))
    );
    let ambiguous = db::WorkUnitRepo::get_by_id(&db, &ambiguous_actor_work_unit_id)
        .await
        .expect("ambiguous provenance loads")
        .expect("ambiguous WorkUnit exists");
    assert_eq!(
        ambiguous.provenance,
        Some(db::WorkUnitProvenance::LegacyActor(ambiguous_actor_id))
    );

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}

#[tokio::test]
async fn cursor_executor_backfill_runs_when_version_53_was_used_by_old_migration() {
    let migration_dir = unique_temp_path("cursor-backfill-migrations");
    fs::create_dir_all(&migration_dir).expect("temp migration dir creates");
    copy_migrations_up_to(52, &migration_dir);

    let db_path = unique_temp_path("cursor-backfill-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");

    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("baseline migrations apply");

    sqlx::query(
        "INSERT INTO _migration (version, name, applied_at) VALUES (53, 'integration_credentials', '2026-05-25T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("old conflicting migration marker inserts");

    run_migrations(&pool)
        .await
        .expect("current migrations backfill cursor executor type");

    let migration_name: String =
        sqlx::query_scalar("SELECT name FROM _migration WHERE version = 54")
            .fetch_one(&pool)
            .await
            .expect("V054 migration applied");
    assert_eq!(migration_name, "cursor_executor_type_backfill");

    AgentRepo::create(
        &SqliteDb::new(pool.clone()),
        CreateAgent {
            id: "cursor-agent".to_owned(),
            name: "Cursor".to_owned(),
            description: None,
            executor_type: "cursor".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            created_at: "now".to_owned(),
            updated_at: "now".to_owned(),
        },
    )
    .await
    .expect("cursor executor type is accepted");

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}

#[tokio::test]
async fn agent_identity_profile_membership_migration_preserves_operational_history() {
    let migration_dir = unique_temp_path("agent-identity-baseline-migrations");
    fs::create_dir_all(&migration_dir).expect("temp migration dir creates");
    copy_migrations_up_to(58, &migration_dir);

    let db_path = unique_temp_path("agent-identity-baseline-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("baseline migrations apply");

    let now = "2026-08-12T00:00:00Z";
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at) VALUES ('project-1', 'Forge', '{}', '{}', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("project inserts");
    sqlx::query(
        "INSERT INTO agent (id, name, description, executor_type, model, reasoning_effort, permission_policy, prompt_template, capabilities_json, config_json, max_concurrent_tasks, status, is_default, paused, owner_id, visibility, created_at, updated_at) VALUES ('agent-1', 'Steward', 'durable teammate', 'codex', 'gpt-5.6', 'high', 'read-only', 'keep scope', '[\"rust\"]', '{\"sandbox\":\"read-only\"}', 2, 'idle', 1, 0, 'user-1', 'account', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy agent inserts");
    sqlx::query(
        "INSERT INTO project_agent_link (id, project_id, agent_id, linked_by_user_id, created_at, updated_at) VALUES ('link-1', 'project-1', 'agent-1', 'user-1', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy project link inserts");
    sqlx::query(
        "INSERT INTO task (id, project_id, title, assignee_type, assignee_id, created_at, updated_at) VALUES ('task-1', 'project-1', 'Preserve me', 'agent', 'agent-1', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("task inserts");
    sqlx::query(
        "INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at) VALUES ('role-1', 'task-1', 'worker', 'agent', 'agent-1', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("role assignment inserts");
    sqlx::query(
        "INSERT INTO execution (id, task_id, agent_id, role, status, agent_session_id, summary, created_at, updated_at) VALUES ('execution-1', 'task-1', 'agent-1', 'executor', 'completed', 'task-session-1', 'delivered', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("execution inserts");
    sqlx::query(
        "INSERT INTO conversation (id, project_id, agent_id, title, system_prompt, message_count, last_message_at, agent_session_id, created_at, updated_at) VALUES ('conversation-1', 'project-1', 'agent-1', 'History', 'stay read only', 2, ?, 'room-session-1', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("conversation inserts");
    sqlx::query(
        "INSERT INTO conversation_message (id, conversation_id, role, content, status, sequence, created_at, updated_at) VALUES ('message-1', 'conversation-1', 'user', 'remember this', 'complete', 1, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("message inserts");
    sqlx::query(
        "INSERT INTO conversation_message (id, conversation_id, role, content, status, model, token_usage_json, duration_ms, error, sequence, created_at, updated_at) VALUES ('message-2', 'conversation-1', 'assistant', 'partial answer', 'streaming', 'gpt-5.6', '{\"input_tokens\":12}', 45, NULL, 2, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("streaming assistant message inserts");
    sqlx::query(
        "INSERT INTO memory_item (id, project_id, task_id, execution_id, conversation_id, source_type, kind, title, body, created_at) VALUES ('memory-1', 'project-1', 'task-1', 'execution-1', 'conversation-1', 'conversation_message', 'observation', 'Remembered', 'remember this', ?)",
    )
    .bind(now)
    .execute(&pool)
    .await
    .expect("memory inserts");
    sqlx::query(
        "INSERT INTO memory_item (id, project_id, conversation_id, source_type, kind, title, body, created_at) VALUES ('memory-secret', 'project-1', 'conversation-1', 'conversation_message', 'observation', 'Secret legacy note', 'Authorization: Bearer sk-test-secret', ?)",
    )
    .bind(now)
    .execute(&pool)
    .await
    .expect("secret memory inserts");

    run_migrations(&pool)
        .await
        .expect("identity/profile migration applies");

    let legacy_agent_table_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent'",
    )
    .fetch_one(&pool)
    .await
    .expect("legacy table check runs");
    assert_eq!(legacy_agent_table_count, 0);
    for table in [
        "room",
        "room_message",
        "agent_turn_job",
        "project_agent_membership",
        "task_role_assignment",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("retired table check runs");
        assert_eq!(count, 0, "legacy runtime table remains live: {table}");
    }

    let identity: (String, String, Option<String>) = sqlx::query_as(
        "SELECT id, name, selected_profile_id FROM agent_identity WHERE id = 'agent-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("identity loads");
    assert_eq!(identity.0, "agent-1");
    assert_eq!(identity.1, "Steward");
    let profile_id = identity.2.expect("selected profile preserved");

    let profile: (String, String, Option<String>, String) = sqlx::query_as(
        "SELECT backend_kind, executor_type, model, capabilities_json FROM agent_profile WHERE id = ? AND identity_id = 'agent-1'",
    )
    .bind(&profile_id)
    .fetch_one(&pool)
    .await
    .expect("profile loads");
    assert_eq!(profile.0, "cli");
    assert_eq!(profile.1, "codex");
    assert_eq!(profile.2.as_deref(), Some("gpt-5.6"));
    assert_eq!(profile.3, "[\"rust\"]");

    let membership: (String, String, String, i64) = sqlx::query_as(
        "SELECT id, identity_id, state, version FROM legacy_project_agent_membership WHERE project_id = 'project-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("membership loads");
    assert_eq!(
        membership,
        (
            "link-1".to_owned(),
            "agent-1".to_owned(),
            "active".to_owned(),
            1
        )
    );

    let historical_membership: (String, String, String) = sqlx::query_as(
        "SELECT rm.id, rm.actor_id, rm.status
         FROM role_membership rm
         JOIN task_role tr ON tr.id = rm.task_role_id
         WHERE tr.task_id = 'task-1' AND tr.role = 'implementer'
           AND rm.actor_kind = 'agent'",
    )
    .fetch_one(&pool)
    .await
    .expect("target membership history loads");
    let membership_issue: String = sqlx::query_scalar(
        "SELECT details_json FROM pr11_vertical_migration_issue
         WHERE source_kind = 'role_membership' AND source_id = ?",
    )
    .bind(&historical_membership.0)
    .fetch_one(&pool)
    .await
    .expect("PR11 migration provenance loads");
    let execution_agent: Option<String> =
        sqlx::query_scalar("SELECT agent_id FROM execution WHERE id = 'execution-1'")
            .fetch_one(&pool)
            .await
            .expect("execution agent loads");
    let execution_session: Option<String> =
        sqlx::query_scalar("SELECT external_session_id FROM pr13_execution_session_projection_history WHERE execution_id = 'execution-1'")
            .fetch_one(&pool)
            .await
            .expect("execution session provenance loads");
    let room_responder: Option<String> = sqlx::query_scalar(
        "SELECT default_responder_identity_id FROM legacy_room WHERE id = 'conversation-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Room responder loads");
    let message_body: String =
        sqlx::query_scalar("SELECT content FROM legacy_room_message WHERE id = 'message-1'")
            .fetch_one(&pool)
            .await
            .expect("message loads");
    let interrupted_message: (String, String, Option<String>, Option<String>, Option<i64>, Option<String>) =
        sqlx::query_as(
            "SELECT status, outcome, model, token_usage_json, duration_ms, error FROM legacy_room_message WHERE id = 'message-2'",
        )
        .fetch_one(&pool)
        .await
        .expect("interrupted message loads");
    let protected_room_session: String = sqlx::query_scalar(
        "SELECT opaque_session_ref FROM protected_legacy_session_ref WHERE room_id = 'conversation-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("protected legacy Room session loads");
    let memory: (String, Option<String>) =
        sqlx::query_as("SELECT body, room_id FROM memory_item WHERE id = 'memory-1'")
            .fetch_one(&pool)
            .await
            .expect("memory loads");
    let secret_memory: (String, String) =
        sqlx::query_as("SELECT body, sensitivity FROM memory_item WHERE id = 'memory-secret'")
            .fetch_one(&pool)
            .await
            .expect("secret memory loads");
    let secret_audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM content_guard_audit WHERE entity_type = 'memory_item' AND entity_id = 'memory-secret'",
    )
    .fetch_one(&pool)
    .await
    .expect("secret memory audit loads");
    let migrated_room_events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE entity_type = 'room_message' AND entity_id = 'message-1'",
    )
    .fetch_one(&pool)
    .await
    .expect("Room events load");
    assert_eq!(historical_membership.1, "agent-1");
    assert_eq!(historical_membership.2, "ended");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&membership_issue)
            .expect("migration provenance is JSON")["agent_id"],
        "agent-1"
    );
    let archived_assignment: (String, String, String, Option<String>, Option<String>) =
        sqlx::query_as(
            "SELECT id, task_id, role_name, assignee_type, assignee_id
             FROM task_role_assignment_history WHERE id = 'role-1'",
        )
        .fetch_one(&pool)
        .await
        .expect("singleton provenance remains as history");
    assert_eq!(
        archived_assignment,
        (
            "role-1".to_owned(),
            "task-1".to_owned(),
            "worker".to_owned(),
            None,
            None,
        )
    );
    assert_eq!(execution_agent.as_deref(), Some("agent-1"));
    assert_eq!(execution_session.as_deref(), Some("task-session-1"));
    assert_eq!(room_responder.as_deref(), Some("agent-1"));
    assert_eq!(message_body, "remember this");
    assert_eq!(interrupted_message.0, "failed");
    assert_eq!(interrupted_message.1, "interrupted_migration");
    assert_eq!(interrupted_message.2.as_deref(), Some("gpt-5.6"));
    assert_eq!(
        interrupted_message.3.as_deref(),
        Some("{\"input_tokens\":12}")
    );
    assert_eq!(interrupted_message.4, Some(45));
    assert_eq!(
        interrupted_message.5.as_deref(),
        Some("interrupted during scoped Room migration")
    );
    assert_eq!(protected_room_session, "room-session-1");
    assert_eq!(memory.0, "remember this");
    assert_eq!(memory.1.as_deref(), Some("conversation-1"));
    assert_eq!(
        secret_memory.0,
        "[protected value redacted during migration]"
    );
    assert_eq!(secret_memory.1, "restricted");
    assert_eq!(secret_audit_count, 1);
    assert_eq!(migrated_room_events, 1);

    let immutable_update = sqlx::query("UPDATE agent_profile SET model = 'changed' WHERE id = ?")
        .bind(&profile_id)
        .execute(&pool)
        .await;
    assert!(immutable_update.is_err());

    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign key check runs");
    assert!(foreign_key_violations.is_empty());

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}

#[tokio::test]
async fn discovery_task_type_migration_preserves_rows_and_constraints() {
    let migration_dir = unique_temp_path("discovery-task-migrations");
    fs::create_dir_all(&migration_dir).expect("temp migration dir creates");
    copy_migrations_up_to(72, &migration_dir);

    let db_path = unique_temp_path("discovery-task-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("pre-discovery migrations apply");

    let now = "2026-08-13T00:00:00Z";
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at) VALUES ('discovery-project', 'Discovery', '{}', '{}', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("project inserts");
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at) VALUES ('discovery-repo', 'discovery-project', 'repo', 'https://example.test/discovery.git', NULL, 'direct_merge', 'main', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("repo inserts");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at) VALUES ('legacy-task', 'discovery-project', 'discovery-repo', 'Legacy task', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy task inserts");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, task_type, created_at, updated_at) VALUES ('legacy-plan', 'discovery-project', 'discovery-repo', 'Legacy plan', 'planning_task', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy planning task inserts");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, parent_task_id, title, task_type, created_at, updated_at) VALUES ('legacy-child', 'discovery-project', 'discovery-repo', 'legacy-task', 'Legacy child', 'sub_task', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy subtask inserts");

    run_migrations(&pool)
        .await
        .expect("discovery task migration applies");
    let db = SqliteDb::new(pool.clone());

    let legacy = TaskRepo::get_by_id(&db, "legacy-task", true)
        .await
        .expect("legacy task loads")
        .expect("legacy task survives");
    assert_eq!(legacy.title, "Legacy task");
    assert_eq!(legacy.task_type, "implementation");
    let legacy_plan = TaskRepo::get_by_id(&db, "legacy-plan", true)
        .await
        .expect("legacy plan loads")
        .expect("legacy plan survives");
    assert_eq!(legacy_plan.task_type, "planning");
    let legacy_child = TaskRepo::get_by_id(&db, "legacy-child", true)
        .await
        .expect("legacy child loads")
        .expect("legacy child survives");
    assert_eq!(legacy_child.task_type, "implementation");
    assert_eq!(legacy_child.parent_task_id.as_deref(), Some("legacy-task"));

    let discovery = TaskRepo::create(
        &db,
        CreateTask {
            id: "discovery-task".to_owned(),
            project_id: "discovery-project".to_owned(),
            repo_id: Some("discovery-repo".to_owned()),
            parent_task_id: None,
            title: "Discover product direction".to_owned(),
            description: Some("Genesis discovery".to_owned()),
            task_type: "discovery".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .expect("discovery task creates through repository");
    assert_eq!(discovery.task_type, "discovery");

    let task_sql: String =
        sqlx::query_scalar("SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'task'")
            .fetch_one(&pool)
            .await
            .expect("task schema loads");
    for task_type in [
        "'implementation'",
        "'planning'",
        "'discovery'",
        "'review'",
        "'validation'",
    ] {
        assert!(
            task_sql.contains(task_type),
            "missing semantic type {task_type}"
        );
    }
    for object in [
        "idx_task_status_project",
        "idx_task_parent",
        "idx_task_repo",
        "idx_task_parent_subtask_order",
        "idx_task_project_archived",
        "idx_task_project_automation",
        "task_board_revision_after_insert",
        "task_board_revision_after_delete",
        "task_board_revision_after_update",
    ] {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = ?")
            .bind(object)
            .fetch_one(&pool)
            .await
            .expect("schema object lookup");
        assert_eq!(count, 1, "schema object {object} should survive rebuild");
    }
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign key check runs");
    assert!(foreign_key_violations.is_empty());

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}

#[tokio::test]
async fn pr13_archives_legacy_agent_context_before_dropping_tables() {
    let historical_dir = unique_temp_path("pr13-v117-migrations");
    let baseline_dir = unique_temp_path("pr13-v120-migrations");
    let final_dir = unique_temp_path("pr13-v132-migrations");
    fs::create_dir_all(&historical_dir).expect("V117 migration dir");
    fs::create_dir_all(&baseline_dir).expect("V120 migration dir");
    fs::create_dir_all(&final_dir).expect("V132 migration dir");
    copy_migrations_up_to(117, &historical_dir);
    copy_migrations_up_to(120, &baseline_dir);
    copy_migrations_up_to(132, &final_dir);

    let db_path = unique_temp_path("pr13-legacy-agent-context-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations_from(&pool, &historical_dir)
        .await
        .expect("V117 historical baseline applies");
    let db = SqliteDb::new(pool.clone());
    let now = "2026-08-13T00:00:00Z";
    let account_id = "pr13-legacy-account";
    let message_id = "pr13-legacy-chat-message";
    let memory_id = "pr13-legacy-memory";
    let content = "legacy message bytes with { raw JSON text }";
    let raw_provenance = "{ \"source\" : \"legacy\", \"revision\" : 7 }";
    let raw_metadata = "[\"legacy\", { \"opaque\" : true }]";

    UserRepo::create_user(
        &db,
        &User {
            id: account_id.to_owned(),
            email: "pr13-legacy@example.test".to_owned(),
            password_hash: "test".to_owned(),
            display_name: None,
            is_admin: false,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .expect("account creates");

    let chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE account_id = ? AND kind = 'account_main'",
    )
    .bind(account_id)
    .fetch_one(&pool)
    .await
    .expect("legacy account chat exists");

    sqlx::query(
        "INSERT INTO agent_chat_message (
            id, chat_id, sequence, author_type, content, status, correlation_id, created_at
         ) VALUES (?, ?, 0, 'user', ?, 'complete', 'legacy-correlation', ?)",
    )
    .bind(message_id)
    .bind(&chat_id)
    .bind(content)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy chat message fixture inserts");
    sqlx::query(
        "INSERT INTO memory_item (
            id, scope_type, scope_id, visibility, provenance_json, source_type,
            kind, title, body, metadata_json, created_at
         ) VALUES (?, 'account', ?, 'account', ?, 'native', 'observation',
                   'Legacy memory', 'preserve exact body', ?, ?)",
    )
    .bind(memory_id)
    .bind(account_id)
    .bind(raw_provenance)
    .bind(raw_metadata)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy memory fixture inserts");

    run_migrations_from(&pool, &baseline_dir)
        .await
        .expect("V118 through V120 upgrade historical database");

    let expected_chat: String = sqlx::query_scalar(
        "SELECT json_object(
            'id', id, 'kind', kind, 'account_id', account_id, 'project_id', project_id,
            'status', status, 'instruction_revision', instruction_revision,
            'message_count', message_count, 'last_message_at', last_message_at,
            'version', version, 'created_at', created_at, 'updated_at', updated_at
         ) FROM agent_chat WHERE id = ?",
    )
    .bind(&chat_id)
    .fetch_one(&pool)
    .await
    .expect("chat snapshot captured");
    let expected_message: String = sqlx::query_scalar(
        "SELECT json_object(
            'id', id, 'chat_id', chat_id, 'sequence', sequence,
            'author_type', author_type, 'author_id', author_id, 'content', content,
            'content_guard_json', content_guard_json, 'sensitivity', sensitivity,
            'status', status, 'outcome', outcome, 'model', model, 'profile_id', profile_id,
            'session_id', session_id, 'context_manifest_id', context_manifest_id,
            'token_usage_json', token_usage_json, 'duration_ms', duration_ms, 'error', error,
            'correlation_id', correlation_id, 'causation_id', causation_id,
            'handoff_id', handoff_id, 'source_type', source_type, 'source_id', source_id,
            'source_message_id', source_message_id, 'source_room_id', source_room_id,
            'source_conversation_id', source_conversation_id,
            'source_sequence', source_sequence, 'source_metadata_json', source_metadata_json,
            'created_at', created_at
         ) FROM agent_chat_message WHERE id = ?",
    )
    .bind(message_id)
    .fetch_one(&pool)
    .await
    .expect("chat message snapshot captured");

    run_migrations_from(&pool, &final_dir)
        .await
        .expect("PR13 migrations apply");

    for table in [
        "agent_chat",
        "agent_chat_message",
        "agent_context_scope",
        "memory_item",
        "agent_lcm_timeline",
        "project_charter",
        "project_milestone",
        "project_readiness_snapshot",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("table presence query");
        assert_eq!(count, 0, "legacy table {table} is physically dropped");
    }

    for (source_table, source_identity, expected) in [
        ("agent_chat", chat_id.as_str(), expected_chat),
        ("agent_chat_message", message_id, expected_message),
    ] {
        let archived: String = sqlx::query_scalar(
            "SELECT row_json FROM pr13_legacy_record_history
             WHERE source_table = ? AND source_identity = ?",
        )
        .bind(source_table)
        .bind(source_identity)
        .fetch_one(&pool)
        .await
        .expect("legacy row archived");
        assert_eq!(archived, expected, "{source_table} snapshot stays exact");
    }
    let archived_memory: String = sqlx::query_scalar(
        "SELECT row_json FROM pr13_legacy_record_history
         WHERE source_table = 'memory_item' AND json_extract(row_json, '$.id') = ?",
    )
    .bind(memory_id)
    .fetch_one(&pool)
    .await
    .expect("legacy memory archived");
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT json_extract(?, '$.body')")
            .bind(&archived_memory)
            .fetch_one(&pool)
            .await
            .expect("memory body extracted"),
        "preserve exact body"
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT json_extract(?, '$.provenance_json')")
            .bind(&archived_memory)
            .fetch_one(&pool)
            .await
            .expect("memory provenance extracted"),
        raw_provenance
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT json_extract(?, '$.metadata_json')")
            .bind(&archived_memory)
            .fetch_one(&pool)
            .await
            .expect("memory metadata extracted"),
        raw_metadata
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity check runs");
    assert_eq!(integrity, "ok");
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign key check runs");
    assert!(foreign_key_violations.is_empty());

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(historical_dir);
    let _ = fs::remove_dir_all(baseline_dir);
    let _ = fs::remove_dir_all(final_dir);
}

#[tokio::test]
async fn v114_preserves_legacy_pr_outcomes_without_granting_admission_authority() {
    let base_dir = unique_temp_path("pr9-v108-migrations");
    let final_dir = unique_temp_path("pr9-v114-migrations");
    fs::create_dir_all(&base_dir).expect("base migration dir");
    fs::create_dir_all(&final_dir).expect("final migration dir");
    copy_migrations_up_to(108, &base_dir);
    copy_migrations_up_to(114, &final_dir);

    let db_path = unique_temp_path("pr9-v109-existing-pr").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations_from(&pool, &base_dir)
        .await
        .expect("V108 baseline");
    let db = SqliteDb::new(pool.clone());
    let now = db::now_rfc3339();
    let project_id = db::new_uuid_v4();
    let repo_id = db::new_uuid_v4();
    let task_ids = [
        db::new_uuid_v4(),
        db::new_uuid_v4(),
        db::new_uuid_v4(),
        db::new_uuid_v4(),
    ];
    db::ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Existing PR migration".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project");
    db::RepoRepo::create(
        &db,
        db::CreateRepo {
            id: repo_id.clone(),
            project_id,
            name: "existing-pr-repo".to_owned(),
            remote_url: "https://example.invalid/existing-pr.git".to_owned(),
            local_path: None,
            work_mode: db::WorkMode::PullRequest,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Repo");
    let repo_project_id: String = sqlx::query_scalar("SELECT project_id FROM repo WHERE id = ?")
        .bind(&repo_id)
        .fetch_one(&pool)
        .await
        .expect("repo project id");
    let cases = [
        ("merged", "merged"),
        ("closed", "closed_without_merge"),
        ("open", "pending"),
        // V109 replaces this old status with the legacy marker, and V113's
        // generic fallback loses it. V114 restores only this recognized
        // terminal provider state from pr_state.
        ("failed", "publication_failed"),
    ];
    let mut metadata_ids = Vec::new();
    for (index, (pr_state, merge_status)) in cases.into_iter().enumerate() {
        let task_id = &task_ids[index];
        TaskRepo::create(
            &db,
            CreateTask {
                id: task_id.clone(),
                project_id: repo_project_id.clone(),
                repo_id: Some(repo_id.clone()),
                parent_task_id: None,
                title: format!("Task with historical {pr_state} PR"),
                description: None,
                task_type: "implementation".to_owned(),
                lifecycle_state: db::TaskLifecycleState::Ready,
                is_automation: false,
                priority: 0,
                subtask_order: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");
        let metadata_id = db::new_uuid_v4();
        let provider_pr_id = format!("old-provider-pr-{index}");
        let pr_url = format!("https://example.invalid/pr/{index}");
        sqlx::query(
            "INSERT INTO pr_metadata (
                id, task_id, provider_type, provider_pr_id, pr_url,
                source_branch, target_branch, pr_state, merge_status,
                last_synced_at, created_at, updated_at
             ) VALUES (?, ?, 'github', ?, ?, 'task/existing', 'main', ?, ?, NULL, ?, ?)",
        )
        .bind(&metadata_id)
        .bind(task_id)
        .bind(&provider_pr_id)
        .bind(&pr_url)
        .bind(pr_state)
        .bind(merge_status)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("pre-V109 provider PR metadata");
        metadata_ids.push((metadata_id, pr_state, merge_status, provider_pr_id, pr_url));
    }

    run_migrations_from(&pool, &final_dir)
        .await
        .expect("upgrade from V108 through V114");
    for (metadata_id, pr_state, merge_status, provider_pr_id, pr_url) in metadata_ids {
        let legacy: (
            String,
            String,
            Option<String>,
            Option<String>,
            String,
            String,
        ) = sqlx::query_as(
            "SELECT merge_status, admission_status, task_merge_operation_id,
                                   publish_operation_id, provider_pr_id, pr_url
                            FROM pr_metadata WHERE id = ?",
        )
        .bind(&metadata_id)
        .fetch_one(&pool)
        .await
        .expect("existing PR remains queryable");
        let expected_merge_status = match pr_state {
            "merged" => "merged",
            "closed" => "closed_without_merge",
            "failed" => "publication_failed",
            _ => "pending",
        };
        assert_eq!(
            legacy.0, expected_merge_status,
            "{pr_state} outcome retained"
        );
        assert_eq!(legacy.1, "legacy_unadmitted");
        assert_eq!(legacy.2, None);
        assert_eq!(legacy.3, None);
        assert_eq!(legacy.4, provider_pr_id);
        assert_eq!(legacy.5, pr_url);
        assert_eq!(merge_status, expected_merge_status);
    }
    let retroactive_admissions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_admission")
            .fetch_one(&pool)
            .await
            .expect("legacy PRs do not gain remote admission rows");
    assert_eq!(retroactive_admissions, 0);

    pool.close().await;
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(base_dir);
    let _ = fs::remove_dir_all(final_dir);
}

#[tokio::test]
async fn v114_recovers_v110_terminal_pr_outcome_from_its_exact_result_event() {
    let v108_dir = unique_temp_path("pr9-v110-crash-v108");
    let v110_dir = unique_temp_path("pr9-v110-crash-v110");
    let v114_dir = unique_temp_path("pr9-v110-crash-v114");
    fs::create_dir_all(&v108_dir).expect("V108 migration dir");
    fs::create_dir_all(&v110_dir).expect("V110 migration dir");
    fs::create_dir_all(&v114_dir).expect("V114 migration dir");
    copy_migrations_up_to(108, &v108_dir);
    copy_migrations_up_to(110, &v110_dir);
    copy_migrations_up_to(114, &v114_dir);

    let db_path = unique_temp_path("pr9-v110-crash-window").with_extension("db");
    let pool = create_sqlite_pool(&format!("sqlite://{}", db_path.display()))
        .await
        .expect("file-backed pool");
    run_migrations_from(&pool, &v108_dir)
        .await
        .expect("V108 baseline");
    let db = SqliteDb::new(pool.clone());
    let now = db::now_rfc3339();
    let project_id = db::new_uuid_v4();
    let repo_id = db::new_uuid_v4();
    let task_id = db::new_uuid_v4();
    let metadata_id = db::new_uuid_v4();
    let merge_id = db::new_uuid_v4();
    let publish_id = db::new_uuid_v4();
    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "V110 terminal PR recovery".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project");
    db::RepoRepo::create(
        &db,
        db::CreateRepo {
            id: repo_id.clone(),
            project_id: project_id.clone(),
            name: "v110-pr-repo".to_owned(),
            remote_url: "https://example.invalid/v110-pr.git".to_owned(),
            local_path: None,
            work_mode: db::WorkMode::PullRequest,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("PullRequest Repo");
    TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: Some(repo_id),
            parent_task_id: None,
            title: "V110 PR merge".to_owned(),
            description: None,
            task_type: "implementation".to_owned(),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Task");
    sqlx::query(
        "INSERT INTO pr_metadata (
            id, task_id, provider_type, provider_pr_id, pr_url, source_branch,
            target_branch, pr_state, merge_status, last_synced_at, created_at,
            updated_at
         ) VALUES (?, ?, 'github', 'existing-v110-pr',
                   'https://example.invalid/pr/v110', 'task/v110', 'main',
                   'open', 'pending', NULL, ?, ?)",
    )
    .bind(&metadata_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("pre-V109 PR metadata");

    run_migrations_from(&pool, &v110_dir)
        .await
        .expect("upgrade through V110");

    // Model the V110 crash window after its exact provider event and terminal
    // operation write, before the separate PR metadata update ran. Drop only
    // the product write guards while constructing this already-committed old
    // state; V113 then has to trust the stored exact operation/event links.
    let trigger_names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type = 'trigger'
           AND tbl_name IN ('task_integration_operation', 'pr_metadata')",
    )
    .fetch_all(&pool)
    .await
    .expect("legacy PR write guards");
    for trigger_name in trigger_names {
        if trigger_name == "task_integration_operation_provider_result_guard" {
            continue;
        }
        let escaped = trigger_name.replace('"', "\"\"");
        sqlx::query(&format!("DROP TRIGGER \"{escaped}\""))
            .execute(&pool)
            .await
            .expect("drop guard for historical crash fixture");
    }
    let status_event = DomainEventRepo::append_event(
        &db,
        CreateDomainEvent {
            id: db::new_uuid_v4(),
            event_type: "pr.status_changed".to_owned(),
            entity_type: "pr_metadata".to_owned(),
            entity_id: metadata_id.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: task_id.clone(),
            correlation_id: metadata_id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some("v110-exact-merged-result".to_owned()),
            payload_json: serde_json::json!({
                "task_id": task_id,
                "pr_metadata_id": metadata_id,
                "status": "merged",
                "task_merge_operation_id": merge_id,
                "publish_operation_id": publish_id,
            })
            .to_string(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("exact historical provider result event");
    sqlx::query(
        "INSERT INTO task_integration_operation (
            id, task_id, kind, owner_id, status, version, created_at,
            updated_at, finished_at, gate_evaluation_id, remote_waiting,
            parent_operation_id, result_event_id
         ) VALUES (?, ?, 'task_merge', 'v110-fixture', 'succeeded', 2, ?, ?, ?,
                   NULL, 1, NULL, ?)",
    )
    .bind(&merge_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&status_event.id)
    .execute(&pool)
    .await
    .expect("terminal V110 TaskMerge");
    sqlx::query(
        "INSERT INTO task_integration_operation (
            id, task_id, kind, owner_id, status, version, created_at,
            updated_at, finished_at, gate_evaluation_id, remote_waiting,
            parent_operation_id, result_event_id
         ) VALUES (?, ?, 'publish_pr', 'v110-fixture', 'succeeded', 2, ?, ?, ?,
                   NULL, 0, ?, NULL)",
    )
    .bind(&publish_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&merge_id)
    .execute(&pool)
    .await
    .expect("terminal V110 PublishPr");
    sqlx::query(
        "UPDATE pr_metadata
         SET task_merge_operation_id = ?, publish_operation_id = ?
         WHERE id = ?",
    )
    .bind(&merge_id)
    .bind(&publish_id)
    .bind(&metadata_id)
    .execute(&pool)
    .await
    .expect("exact old operation links");
    let before: (String, String) = sqlx::query_as(
        "SELECT merge_op.status, metadata.merge_status
         FROM pr_metadata metadata
         JOIN task_integration_operation merge_op
           ON merge_op.id = metadata.task_merge_operation_id
         WHERE metadata.id = ?",
    )
    .bind(&metadata_id)
    .fetch_one(&pool)
    .await
    .expect("V110 crash-window state");
    assert_eq!(
        before,
        ("succeeded".to_owned(), "legacy_unadmitted".to_owned())
    );

    run_migrations_from(&pool, &v114_dir)
        .await
        .expect("upgrade exact V110 crash window through V114");
    let after: (String, String, String, String) = sqlx::query_as(
        "SELECT merge_op.status, metadata.merge_status, metadata.pr_state,
                metadata.admission_status
         FROM pr_metadata metadata
         JOIN task_integration_operation merge_op
           ON merge_op.id = metadata.task_merge_operation_id
         WHERE metadata.id = ?",
    )
    .bind(&metadata_id)
    .fetch_one(&pool)
    .await
    .expect("legacy terminal result stays queryable");
    assert_eq!(
        after,
        (
            "succeeded".to_owned(),
            "merged".to_owned(),
            "merged".to_owned(),
            "legacy_unadmitted".to_owned(),
        )
    );
    let remote_admissions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remote_pr_admission")
        .fetch_one(&pool)
        .await
        .expect("no retrospective remote admission");
    assert_eq!(remote_admissions, 0);

    pool.close().await;
    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(v108_dir);
    let _ = fs::remove_dir_all(v110_dir);
    let _ = fs::remove_dir_all(v114_dir);
}

#[tokio::test]
async fn v133_follow_up_receipts_upgrade_from_v132_and_restart_cleanly() {
    let baseline_dir = unique_temp_path("v132-follow-up-baseline");
    let final_dir = unique_temp_path("v133-follow-up-upgrade");
    fs::create_dir_all(&baseline_dir).expect("V132 migration dir creates");
    fs::create_dir_all(&final_dir).expect("V133 migration dir creates");
    copy_migrations_up_to(132, &baseline_dir);
    copy_migrations_up_to(133, &final_dir);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &baseline_dir)
        .await
        .expect("database reaches V132");
    let baseline_head: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("V132 head loads");
    assert_eq!(baseline_head, 132);

    run_migrations_from(&pool, &final_dir)
        .await
        .expect("V133 adds follow-up receipts");
    let receipt_head: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("V133 head loads");
    assert_eq!(receipt_head, 133);
    let receipt_table: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='execution_follow_up_receipt')",
    )
    .fetch_one(&pool)
    .await
    .expect("follow-up receipt table lookup");
    assert_eq!(receipt_table, 1);

    let receipt_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("migration receipt count loads");
    run_migrations_from(&pool, &final_dir)
        .await
        .expect("restart does not recreate V133");
    let restarted_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("restarted migration receipt count loads");
    assert_eq!(restarted_count, receipt_count);
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign_key_check runs after V133");
    assert!(foreign_key_violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check runs after V133");
    assert_eq!(integrity, "ok");

    let _ = fs::remove_dir_all(baseline_dir);
    let _ = fs::remove_dir_all(final_dir);
}

#[tokio::test]
async fn v134_replaces_admission_guard_on_v133_database_and_restarts_cleanly() {
    let baseline_dir = unique_temp_path("v133-admission-guard-baseline");
    let final_dir = unique_temp_path("v134-admission-guard-upgrade");
    fs::create_dir_all(&baseline_dir).expect("V133 migration dir creates");
    fs::create_dir_all(&final_dir).expect("V134 migration dir creates");
    copy_migrations_up_to(133, &baseline_dir);
    copy_migrations_up_to(134, &final_dir);

    let db_path = final_dir.join("existing-v133.db");
    let database_url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&database_url).await.expect("pool");
    run_migrations_from(&pool, &baseline_dir)
        .await
        .expect("existing database reaches V133");
    let old_trigger: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'trigger'
         AND name = 'work_unit_execution_admission_guard'",
    )
    .fetch_one(&pool)
    .await
    .expect("V132 admission trigger exists before upgrade");
    assert!(old_trigger.contains("Agent reached concurrent execution capacity"));

    run_migrations_from(&pool, &final_dir)
        .await
        .expect("existing V133 database upgrades to V134");
    let trigger_after_upgrade: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'trigger'
         AND name = 'work_unit_execution_admission_guard'",
    )
    .fetch_one(&pool)
    .await
    .expect("V134 admission trigger exists after upgrade");
    assert!(trigger_after_upgrade.contains("FORGE_WU_ADMISSION_AGENT_AT_CAPACITY"));
    assert!(trigger_after_upgrade.contains("FORGE_WU_ADMISSION_ROLE_MEMBERSHIP_INACTIVE"));
    assert!(!trigger_after_upgrade.contains("Agent reached concurrent execution capacity"));
    let migration_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("migration count loads after V134");
    pool.close().await;

    let restarted_pool = create_sqlite_pool(&database_url)
        .await
        .expect("database reopens after restart");
    run_migrations_from(&restarted_pool, &final_dir)
        .await
        .expect("restart keeps V134 applied");
    let restarted_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&restarted_pool)
        .await
        .expect("migration count loads after restart");
    let restarted_trigger: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'trigger'
         AND name = 'work_unit_execution_admission_guard'",
    )
    .fetch_one(&restarted_pool)
    .await
    .expect("V134 trigger survives restart");
    assert_eq!(restarted_count, migration_count);
    assert_eq!(restarted_trigger, trigger_after_upgrade);
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&restarted_pool)
            .await
            .expect("foreign_key_check runs after V134");
    assert!(foreign_key_violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&restarted_pool)
        .await
        .expect("integrity_check runs after V134");
    assert_eq!(integrity, "ok");
    restarted_pool.close().await;

    let _ = fs::remove_dir_all(baseline_dir);
    let _ = fs::remove_dir_all(final_dir);
}

fn copy_migrations_up_to(max_version: i64, destination: &Path) {
    let source_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in fs::read_dir(source_dir).expect("migration dir reads") {
        let entry = entry.expect("migration entry reads");
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(version) = migration_version(filename) else {
            continue;
        };
        if version <= max_version {
            fs::copy(&path, destination.join(filename)).expect("migration copies");
        }
    }
}

fn migration_version(filename: &str) -> Option<i64> {
    filename.strip_prefix('V')?.split_once("__")?.0.parse().ok()
}

#[tokio::test]
async fn pr13_v129_rejects_and_rolls_back_a_historical_task_without_lifecycle() {
    let baseline_dir = unique_temp_path("pr13-v128-lifecycle-migrations");
    let cutover_dir = unique_temp_path("pr13-v129-lifecycle-migrations");
    fs::create_dir_all(&baseline_dir).expect("V128 migration dir creates");
    fs::create_dir_all(&cutover_dir).expect("V129 migration dir creates");
    copy_migrations_up_to(128, &baseline_dir);
    copy_migrations_up_to(129, &cutover_dir);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &baseline_dir)
        .await
        .expect("V128 baseline applies");
    sqlx::query("DROP TRIGGER task_lifecycle_insert_for_new_task")
        .execute(&pool)
        .await
        .expect("simulate an incomplete historical direct Task writer");
    sqlx::query(
        "INSERT INTO project (id, name, settings, created_at, updated_at)
         VALUES ('pr13-orphan-project', 'Lifecycle cutover', '{}', '2026-10-13T00:00:00Z', '2026-10-13T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("historical Project");
    sqlx::query(
        "INSERT INTO task (id, project_id, title, task_type, status, created_at, updated_at)
         VALUES ('pr13-orphan-task', 'pr13-orphan-project', 'Orphan fixture', 'implementation', 'todo', '2026-10-13T00:00:00Z', '2026-10-13T00:00:00Z')",
    )
    .execute(&pool)
    .await
    .expect("historical Task without lifecycle");

    let cutover = run_migrations_from(&pool, &cutover_dir).await;
    assert!(
        cutover.is_err(),
        "V129 must fail closed on a missing lifecycle"
    );

    let orphan_task_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task WHERE id = 'pr13-orphan-task' AND status = 'todo'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical Task survives failed cutover");
    assert_eq!(orphan_task_count, 1);
    let lifecycle_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM task_lifecycle WHERE task_id = 'pr13-orphan-task'",
    )
    .fetch_one(&pool)
    .await
    .expect("TaskLifecycle remains absent");
    assert_eq!(lifecycle_count, 0);
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _migration WHERE version = 129")
            .fetch_one(&pool)
            .await
            .expect("V129 receipt lookup");
    assert_eq!(receipt_count, 0);
    let status_column: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('task') WHERE name = 'status'")
            .fetch_one(&pool)
            .await
            .expect("legacy Task schema remains after rollback");
    assert_eq!(status_column, 1);
    let lifecycle_trigger: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'trigger' AND name = 'task_lifecycle_insert_for_new_task'",
    )
    .fetch_one(&pool)
    .await
    .expect("pre-cutover TaskLifecycle trigger remains");
    assert_eq!(lifecycle_trigger, 0, "the deliberate fixture drop persists");
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign_key_check runs after rollback");
    assert!(foreign_key_violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check runs after rollback");
    assert_eq!(integrity, "ok");
    let foreign_keys_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("foreign_keys state reads after rollback");
    assert_eq!(foreign_keys_enabled, 1);

    let _ = fs::remove_dir_all(baseline_dir);
    let _ = fs::remove_dir_all(cutover_dir);
}

#[tokio::test]
async fn pr13_lifecycle_cutover_accepts_restart_with_no_new_receipts_or_rows() {
    let migration_dir = unique_temp_path("pr13-v132-lifecycle-restart");
    fs::create_dir_all(&migration_dir).expect("V132 migration dir creates");
    copy_migrations_up_to(132, &migration_dir);
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");

    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("first V1-V132 migration pass");
    let first_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("first migration receipt count");
    let first_lifecycle_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle")
        .fetch_one(&pool)
        .await
        .expect("first lifecycle row count");

    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("restart migration pass is idempotent");
    let second_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("second migration receipt count");
    let second_lifecycle_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_lifecycle")
        .fetch_one(&pool)
        .await
        .expect("second lifecycle row count");
    assert_eq!(second_receipts, first_receipts);
    assert_eq!(second_lifecycle_rows, first_lifecycle_rows);

    let foreign_keys_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("foreign_keys PRAGMA reads after restart");
    let legacy_alter_table: i64 = sqlx::query_scalar("PRAGMA legacy_alter_table")
        .fetch_one(&pool)
        .await
        .expect("legacy_alter_table PRAGMA reads after restart");
    assert_eq!(foreign_keys_enabled, 1);
    assert_eq!(legacy_alter_table, 0);

    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign_key_check runs after restart");
    assert!(foreign_key_violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check runs after restart");
    assert_eq!(integrity, "ok");

    let _ = fs::remove_dir_all(migration_dir);
}

#[tokio::test]
async fn pr13_v120_empty_database_upgrades_and_restarts_cleanly() {
    let baseline_dir = unique_temp_path("pr13-v120-empty-baseline");
    let final_dir = unique_temp_path("pr13-v132-empty-upgrade");
    fs::create_dir_all(&baseline_dir).expect("V120 migration dir creates");
    fs::create_dir_all(&final_dir).expect("V132 migration dir creates");
    copy_migrations_up_to(120, &baseline_dir);
    copy_migrations_up_to(132, &final_dir);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &baseline_dir)
        .await
        .expect("empty database reaches V120");
    let v120_max_version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("V120 migration head loads");
    let v120_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("V120 receipt count loads");
    assert_eq!(v120_max_version, 120);

    run_migrations_from(&pool, &final_dir)
        .await
        .expect("empty V120 database upgrades to V132");
    let first_max_version: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("V132 migration head loads");
    let first_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("V132 receipt count loads");
    assert_eq!(first_max_version, 132);
    assert_eq!(first_receipts, v120_receipts + 12);
    run_migrations_from(&pool, &final_dir)
        .await
        .expect("restart leaves V132 schema unchanged");
    let second_receipts: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("restart receipt count loads");
    assert_eq!(second_receipts, first_receipts);

    let release_history_exists: i64 = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'project_release_history_definition')",
    )
    .fetch_one(&pool)
    .await
    .expect("Release history storage lookup");
    assert_eq!(release_history_exists, 1);
    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("foreign_keys reads after upgrade");
    let legacy_alter_table: i64 = sqlx::query_scalar("PRAGMA legacy_alter_table")
        .fetch_one(&pool)
        .await
        .expect("legacy_alter_table reads after upgrade");
    assert_eq!(foreign_keys, 1);
    assert_eq!(legacy_alter_table, 0);
    let violations: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("foreign_key_check runs after upgrade");
    assert!(violations.is_empty());
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity_check runs after upgrade");
    assert_eq!(integrity, "ok");

    let _ = fs::remove_dir_all(baseline_dir);
    let _ = fs::remove_dir_all(final_dir);
}

#[tokio::test]
async fn singular_agent_chat_migration_merges_threads_and_recovers_turns() {
    let migration_dir = unique_temp_path("singular-chat-matrix-migrations");
    fs::create_dir_all(&migration_dir).expect("temp migration dir creates");
    copy_migrations_up_to(58, &migration_dir);

    let db_path = unique_temp_path("singular-chat-matrix-db").with_extension("db");
    let url = format!("sqlite://{}", db_path.display());
    let pool = create_sqlite_pool(&url).await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("legacy baseline migrations apply");

    let now = "2026-08-13T00:00:00Z";
    let account_id = "matrix-account";
    let merged_project_id = "matrix-merged-project";
    let ambiguous_project_id = "matrix-ambiguous-project";
    let worker_project_id = "matrix-worker-project";
    let turns_project_id = "matrix-turns-project";

    sqlx::query(
        "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES (?, ?, 'test', 'Matrix', ?, ?)",
    )
    .bind(account_id)
    .bind("matrix@example.test")
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("account inserts");

    for (project_id, name) in [
        (merged_project_id, "Merged Project"),
        (ambiguous_project_id, "Ambiguous Project"),
        (worker_project_id, "Worker Project"),
        (turns_project_id, "Turns Project"),
    ] {
        sqlx::query(
            "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
             VALUES (?, ?, '{}', '{}', ?, ?)",
        )
        .bind(project_id)
        .bind(name)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("project inserts");
        sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
            .bind(account_id)
            .bind(project_id)
            .execute(&pool)
            .await
            .expect("project owner assignment");
    }

    for (agent_id, name) in [
        ("matrix-agent-a", "Matrix Agent A"),
        ("matrix-agent-b", "Matrix Agent B"),
        ("matrix-agent-worker", "Matrix Worker"),
    ] {
        sqlx::query(
            "INSERT INTO agent (
                id, name, description, executor_type, model, reasoning_effort,
                permission_policy, prompt_template, capabilities_json, config_json,
                max_concurrent_tasks, status, is_default, paused, owner_id, visibility,
                created_at, updated_at
             ) VALUES (?, ?, NULL, 'null', 'matrix-model', NULL, NULL, NULL, '{}', '{}',
                       1, 'idle', 0, 0, ?, 'account', ?, ?)",
        )
        .bind(agent_id)
        .bind(name)
        .bind(account_id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("legacy agent inserts");
    }

    sqlx::query(
        "INSERT INTO conversation (
            id, project_id, agent_id, title, status, system_prompt, message_count,
            last_message_at, agent_session_id, version, created_at, updated_at
         ) VALUES (
            'conversation-legacy', ?, 'matrix-agent-a', 'Legacy Conversation', 'active',
            'Legacy instruction', 1, ?, 'opaque-legacy-session', 1, ?, ?
         )",
    )
    .bind(merged_project_id)
    .bind("2026-08-13T00:00:02Z")
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy Conversation inserts");
    sqlx::query(
        "INSERT INTO conversation_message (
            id, conversation_id, role, content, status, sequence, created_at, updated_at
         ) VALUES (
            'legacy-message', 'conversation-legacy', 'user', 'legacy conversation body',
            'complete', 0, '2026-08-13T00:00:02Z', '2026-08-13T00:00:02Z'
         )",
    )
    .execute(&pool)
    .await
    .expect("legacy Conversation message inserts");

    copy_migrations_up_to(70, &migration_dir);
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("Room-era migrations apply");

    for (room_id, project_id, responder_id) in [
        ("room-a", merged_project_id, "matrix-agent-a"),
        ("room-z", merged_project_id, "matrix-agent-a"),
        ("ambiguous-a", ambiguous_project_id, "matrix-agent-a"),
        ("ambiguous-b", ambiguous_project_id, "matrix-agent-b"),
        ("worker-room", worker_project_id, "matrix-agent-worker"),
        ("turn-live", turns_project_id, "matrix-agent-a"),
        ("turn-expired", turns_project_id, "matrix-agent-a"),
        ("turn-exhausted", turns_project_id, "matrix-agent-a"),
    ] {
        sqlx::query(
            "INSERT INTO room (
                id, scope_type, scope_id, owner_user_id, owning_project_id, title, status,
                responder_policy, default_responder_identity_id, history_policy,
                message_count, last_message_at, version, created_at, updated_at
             ) VALUES (?, 'project', ?, ?, ?, ?, 'active', 'explicit_identity', ?,
                       'project_members', 0, NULL, 1, ?, ?)",
        )
        .bind(room_id)
        .bind(project_id)
        .bind(account_id)
        .bind(project_id)
        .bind(format!("{room_id} title"))
        .bind(responder_id)
        .bind(now)
        .bind(now)
        .execute(&pool)
        .await
        .expect("pre-singular Room inserts");
    }

    sqlx::query(
        "INSERT INTO room_instruction_revision (
            id, room_id, revision, body, content_guard_json, sensitivity,
            created_by_type, created_by_id, created_at
         ) VALUES ('room-a-instruction', 'room-a', 1, 'Room A instruction', '{}',
                   'internal', 'user', ?, ?)",
    )
    .bind(account_id)
    .bind(now)
    .execute(&pool)
    .await
    .expect("Room instruction inserts");
    sqlx::query(
        "INSERT INTO agent_lcm_timeline (
            id, identity_id, scope_type, scope_id, authorization_revision,
            revision, created_at, updated_at
         ) VALUES ('matrix-room-lcm', 'matrix-agent-a', 'room', 'room-a',
                   'legacy-room-auth', 0, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("legacy Room LCM timeline inserts");

    let room_messages = [
        (
            "a-message-0",
            "room-a",
            "A first body",
            0_i64,
            "2026-08-13T00:00:01Z",
        ),
        (
            "a-message-1",
            "room-a",
            "A second body",
            1_i64,
            "2026-08-13T00:00:01Z",
        ),
        (
            "z-message-0",
            "room-z",
            "Authorization: Bearer sk-matrix-secret",
            0_i64,
            "2026-08-13T00:00:01Z",
        ),
        (
            "ambiguous-message-a",
            "ambiguous-a",
            "Ambiguous A body",
            0_i64,
            "2026-08-13T00:00:03Z",
        ),
        (
            "ambiguous-message-b",
            "ambiguous-b",
            "Ambiguous B body",
            0_i64,
            "2026-08-13T00:00:03Z",
        ),
        (
            "worker-message",
            "worker-room",
            "Primary Worker transcript",
            0_i64,
            "2026-08-13T00:00:04Z",
        ),
        (
            "live-message",
            "turn-live",
            "Live leased input",
            0_i64,
            "2026-08-13T00:00:05Z",
        ),
        (
            "expired-message",
            "turn-expired",
            "Expired leased input",
            0_i64,
            "2026-08-13T00:00:06Z",
        ),
        (
            "exhausted-message",
            "turn-exhausted",
            "Exhausted leased input",
            0_i64,
            "2026-08-13T00:00:07Z",
        ),
    ];
    for (message_id, room_id, content, sequence, created_at) in room_messages {
        sqlx::query(
            "INSERT INTO room_message (
                id, room_id, author_type, author_id, addressed_identity_id,
                reply_to_message_id, content, content_guard_json, sensitivity, status,
                outcome, model, profile_id, session_id, token_usage_json, duration_ms,
                error, correlation_id, source_event_id, sequence, created_at
             ) VALUES (?, ?, 'user', ?, NULL, NULL, ?, '{}', 'internal', 'complete',
                       NULL, NULL, NULL, NULL, NULL, NULL, NULL, ?, NULL, ?, ?)",
        )
        .bind(message_id)
        .bind(room_id)
        .bind(account_id)
        .bind(content)
        .bind(format!("correlation-{message_id}"))
        .bind(sequence)
        .bind(created_at)
        .execute(&pool)
        .await
        .expect("pre-singular Room message inserts");
    }

    sqlx::query(
        "INSERT INTO project_agent_membership (
            id, project_id, identity_id, role, is_primary, state, created_by_user_id,
            created_at, updated_at
         ) VALUES ('worker-membership', ?, 'matrix-agent-worker', 'worker', 1,
                   'active', ?, ?, ?)",
    )
    .bind(worker_project_id)
    .bind(account_id)
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("primary Worker membership inserts");

    for (job_id, room_id, message_id, status, leased_until, attempt_count, updated_at) in [
        (
            "live-job",
            "turn-live",
            "live-message",
            "leased",
            "2099-01-01T00:00:00Z",
            1_i64,
            now,
        ),
        (
            "expired-job",
            "turn-expired",
            "expired-message",
            "running",
            "2026-08-01T00:00:00Z",
            2_i64,
            now,
        ),
        (
            "exhausted-job",
            "turn-exhausted",
            "exhausted-message",
            "leased",
            "2026-08-01T00:00:00Z",
            3_i64,
            now,
        ),
    ] {
        sqlx::query(
            "INSERT INTO agent_turn_job (
                id, room_id, input_message_id, responder_identity_id, scope_type, scope_id,
                status, dedupe_key, lease_owner, leased_until, attempt_count,
                response_message_id, error, correlation_id, causation_id, causation_depth,
                created_at, updated_at
             ) VALUES (?, ?, ?, 'matrix-agent-a', 'room', ?, ?, ?, 'matrix-worker', ?, ?,
                       NULL, NULL, ?, NULL, 0, ?, ?)",
        )
        .bind(job_id)
        .bind(room_id)
        .bind(message_id)
        .bind(room_id)
        .bind(status)
        .bind(format!("dedupe-{job_id}"))
        .bind(leased_until)
        .bind(attempt_count)
        .bind(format!("correlation-{job_id}"))
        .bind(now)
        .bind(updated_at)
        .execute(&pool)
        .await
        .expect("legacy turn job inserts");
    }

    copy_migrations_up_to(120, &migration_dir);
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("PR11-era Agent Chat history and fences apply through V120");

    for table in [
        "room",
        "room_instruction_revision",
        "room_participant",
        "room_message",
        "agent_turn_job",
        "bounded_room_round",
        "bounded_room_round_participant",
        "project_agent_membership",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("live legacy table inspection");
        assert_eq!(
            count, 0,
            "retired runtime table must not remain live: {table}"
        );
    }
    for (table, expected_rows) in [
        ("legacy_room", 9_i64),
        ("legacy_room_instruction_revision", 2_i64),
        ("legacy_room_message", 10_i64),
        ("legacy_agent_turn_job", 3_i64),
        ("legacy_project_agent_membership", 1_i64),
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .expect("quarantined legacy rows load");
        assert_eq!(
            count, expected_rows,
            "quarantined legacy rows are preserved: {table}"
        );
    }

    let merged_chat_id: String =
        sqlx::query_scalar("SELECT id FROM agent_chat WHERE kind = 'project' AND project_id = ?")
            .bind(merged_project_id)
            .fetch_one(&pool)
            .await
            .expect("merged Project Chat lookup");
    let merged_messages: Vec<(String, String, String, i64, String)> = sqlx::query_as(
        "SELECT id, content, source_room_id, source_sequence, source_type
         FROM agent_chat_message WHERE chat_id = ? ORDER BY sequence ASC",
    )
    .bind(&merged_chat_id)
    .fetch_all(&pool)
    .await
    .expect("merged message lookup");
    assert_eq!(
        merged_messages,
        vec![
            (
                "a-message-0".to_owned(),
                "A first body".to_owned(),
                "room-a".to_owned(),
                0,
                "room".to_owned(),
            ),
            (
                "a-message-1".to_owned(),
                "A second body".to_owned(),
                "room-a".to_owned(),
                1,
                "room".to_owned(),
            ),
            (
                "z-message-0".to_owned(),
                "[protected value redacted during migration]".to_owned(),
                "room-z".to_owned(),
                0,
                "room".to_owned(),
            ),
            (
                "legacy-message".to_owned(),
                "legacy conversation body".to_owned(),
                "conversation-legacy".to_owned(),
                0,
                "room".to_owned(),
            ),
        ]
    );

    let source_ref_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_chat_source_ref
         WHERE chat_id = ? AND source_type = 'room'",
    )
    .bind(&merged_chat_id)
    .fetch_one(&pool)
    .await
    .expect("merged source refs lookup");
    assert_eq!(source_ref_count, 3);
    let lcm_source_ref: (String, String, String) = sqlx::query_as(
        "SELECT source_type, source_scope_type, source_scope_id
         FROM agent_chat_source_ref
         WHERE chat_id = ? AND source_id = 'matrix-room-lcm'",
    )
    .bind(&merged_chat_id)
    .fetch_one(&pool)
    .await
    .expect("legacy LCM source ref lookup");
    assert_eq!(
        lcm_source_ref,
        (
            "lcm_timeline".to_owned(),
            "room".to_owned(),
            "room-a".to_owned()
        )
    );
    let legacy_instruction: (String, String, String) = sqlx::query_as(
        "SELECT source_type, source_id, body FROM agent_chat_instruction_revision
         WHERE chat_id = ? AND source_id = 'conversation-legacy'",
    )
    .bind(&merged_chat_id)
    .fetch_one(&pool)
    .await
    .expect("legacy instruction provenance lookup");
    assert_eq!(
        legacy_instruction,
        (
            "room".to_owned(),
            "conversation-legacy".to_owned(),
            "Legacy instruction".to_owned()
        )
    );

    let protected_session: (String, String) = sqlx::query_as(
        "SELECT room_id, opaque_session_ref FROM protected_legacy_session_ref
         WHERE room_id = 'conversation-legacy'",
    )
    .fetch_one(&pool)
    .await
    .expect("protected legacy session linkage lookup");
    assert_eq!(
        protected_session,
        (
            "conversation-legacy".to_owned(),
            "opaque-legacy-session".to_owned()
        )
    );
    let leaked_session_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_chat_message
         WHERE chat_id = ? AND content LIKE '%opaque-legacy-session%'",
    )
    .bind(&merged_chat_id)
    .fetch_one(&pool)
    .await
    .expect("protected session leak check");
    assert_eq!(leaked_session_count, 0);

    let secret_audit_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM content_guard_audit
         WHERE entity_type = 'agent_chat_message' AND entity_id = 'z-message-0'",
    )
    .fetch_one(&pool)
    .await
    .expect("protected chat audit lookup");
    assert_eq!(secret_audit_count, 1);

    for project_id in [ambiguous_project_id, worker_project_id] {
        let chat_status: String = sqlx::query_scalar(
            "SELECT status FROM agent_chat WHERE kind = 'project' AND project_id = ?",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .expect("setup-required Project Chat lookup");
        assert_eq!(chat_status, "agent_setup_required");
        let binding: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT state, identity_id, profile_id FROM project_agent_binding WHERE project_id = ?",
        )
        .bind(project_id)
        .fetch_one(&pool)
        .await
        .expect("setup-required Project binding lookup");
        assert_eq!(binding, ("agent_setup_required".to_owned(), None, None));
    }
    let worker_binding_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM project_agent_binding
         WHERE project_id = ? AND identity_id = 'matrix-agent-worker' AND state = 'active'",
    )
    .bind(worker_project_id)
    .fetch_one(&pool)
    .await
    .expect("primary Worker binding lookup");
    assert_eq!(worker_binding_count, 0);

    type MigratedTurnState = (String, String, Option<String>, Option<String>, i64);
    let turn_states: Vec<MigratedTurnState> = sqlx::query_as(
        "SELECT id, status, error_code, lease_owner, attempt_count
             FROM agent_chat_turn_job WHERE id IN ('live-job', 'expired-job', 'exhausted-job')
             ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("migrated turn state lookup");
    assert_eq!(
        turn_states,
        vec![
            (
                "exhausted-job".to_owned(),
                "failed".to_owned(),
                Some("retry_exhausted".to_owned()),
                None,
                3,
            ),
            (
                "expired-job".to_owned(),
                "retry_wait".to_owned(),
                Some("lease_expired_during_migration".to_owned()),
                None,
                2,
            ),
            (
                "live-job".to_owned(),
                "leased".to_owned(),
                None,
                Some("matrix-worker".to_owned()),
                1,
            ),
        ]
    );
    let canonical_scope: (String, String) = sqlx::query_as(
        "SELECT canonical_scope_type, canonical_scope_id FROM agent_chat_turn_job
         WHERE id = 'live-job'",
    )
    .fetch_one(&pool)
    .await
    .expect("canonical turn scope lookup");
    assert_eq!(canonical_scope.0, "agent_chat");
    assert_eq!(
        canonical_scope.1,
        merged_chat_id_for(&pool, turns_project_id).await
    );

    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign key check runs");
    assert!(foreign_key_violations.is_empty());

    let _ = fs::remove_file(db_path);
    let _ = fs::remove_dir_all(migration_dir);
}

async fn merged_chat_id_for(pool: &db::SqlitePool, project_id: &str) -> String {
    sqlx::query_scalar("SELECT id FROM agent_chat WHERE kind = 'project' AND project_id = ?")
        .bind(project_id)
        .fetch_one(pool)
        .await
        .expect("Project Chat id lookup")
}

#[tokio::test]
async fn pr13_fresh_database_drops_legacy_storage_and_keeps_exact_builtin_skill_archive() {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");

    for table in ["user", "project", "agent_identity", "agent_profile"] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("target storage presence lookup");
        assert_eq!(count, 1, "target storage table remains: {table}");
    }

    for table in [
        "account_main_agent_binding",
        "project_agent_binding",
        "agent_chat",
        "agent_chat_message",
        "agent_chat_turn_job",
        "agent_chat_source_ref",
        "memory_item",
        "operating_skill",
        "operating_skill_revision",
        "project_charter",
        "project_milestone",
        "project_readiness_snapshot",
    ] {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("legacy table presence lookup");
        assert_eq!(count, 0, "fresh schema must drop legacy table {table}");
    }
    let schema_objects: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT type, name, lower(sql) FROM sqlite_master
         WHERE type IN ('trigger', 'index', 'view') AND sql IS NOT NULL",
    )
    .fetch_all(&pool)
    .await
    .expect("remaining trigger, index and view definitions load");
    for retired_name in [
        "account_main_agent_binding",
        "agent_action",
        "agent_chat",
        "agent_context_scope",
        "attention_projection",
        "legacy_room",
        "memory_item",
        "operating_skill",
        "product_genesis_session",
        "project_agent_binding",
        "project_charter",
        "project_document",
        "project_execution_baseline",
        "project_milestone",
        "project_readiness_snapshot",
        "project_media_attachment",
        "protected_legacy_session_ref",
    ] {
        let references: Vec<_> = schema_objects
            .iter()
            .filter(|(_, _, sql)| sql.contains(retired_name))
            .collect();
        assert!(
            references.is_empty(),
            "retained schema object references retired table {retired_name}: {references:?}"
        );
    }
    for retired_column in [
        "agent_session_id",
        "assignee_type",
        "assignee_id",
        "task_state_config",
        "entry_barrier_json",
        "review_passed_at",
        "default_assignee_type",
        "default_assignee_id",
        "workflow_definition",
        "workflow_template_name",
        "charter_status",
        "charter_setup_required",
        "current_charter_id",
        "current_charter_revision_id",
        "current_charter_version",
        "primary_milestone_id",
    ] {
        let references: Vec<_> = schema_objects
            .iter()
            .filter(|(_, _, sql)| sql.contains(retired_column))
            .collect();
        assert!(
            references.is_empty(),
            "retained schema object references retired column {retired_column}: {references:?}"
        );
    }
    let legacy_sources: Vec<(String, i64)> = sqlx::query_as(
        "SELECT source_table, COUNT(*) FROM pr13_legacy_record_history GROUP BY source_table ORDER BY source_table",
    )
    .fetch_all(&pool)
    .await
    .expect("migration-only legacy archive sources");
    assert_eq!(
        legacy_sources,
        vec![
            ("operating_skill".to_owned(), 2),
            ("operating_skill_revision".to_owned(), 2),
        ],
        "fresh DB archives only the two exact V076 built-in operating skills and revisions"
    );
    let legacy_identities: Vec<(String, String)> = sqlx::query_as(
        "SELECT source_table, source_identity FROM pr13_legacy_record_history
         ORDER BY source_table, source_identity",
    )
    .fetch_all(&pool)
    .await
    .expect("built-in skill archive identities");
    assert_eq!(
        legacy_identities,
        vec![
            (
                "operating_skill".to_owned(),
                "forge.main.project-discovery/v2".to_owned(),
            ),
            (
                "operating_skill".to_owned(),
                "forge.project.orchestration/v1".to_owned(),
            ),
            (
                "operating_skill_revision".to_owned(),
                "forge.main.project-discovery/v2@1".to_owned(),
            ),
            (
                "operating_skill_revision".to_owned(),
                "forge.project.orchestration/v1@1".to_owned(),
            ),
        ],
        "migration archives the original built-in identities without synthesis"
    );
}
