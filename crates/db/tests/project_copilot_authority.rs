use db::{
    create_sqlite_pool, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
    CopilotMessagePageQuery, CreateAgent, CreateProject, CreateProjectCopilotMessage,
    CreateProjectCopilotRun, CreateProjectMember, CreateTask, CreateTaskRole, DaemonRepo,
    DaemonStatus, ProjectCopilotRepo, ProjectCopilotRunStatus, ProjectMemberRepo, ProjectRepo,
    SqliteDb, TaskLifecycleState, TaskRepo, TaskRoleRepo, UpdateAgent, UpdateDaemonReport,
    UpsertDaemon, User, UserRepo,
};
use sqlx::{Sqlite, Transaction};

struct Fixture {
    db: SqliteDb,
    project_id: String,
    task_id: String,
    owner_id: String,
    caller_id: String,
    transfer_owner_id: String,
    agent_id: String,
    daemon_id: String,
    run_id: String,
}

#[derive(Clone, Copy)]
enum AuthorityChange {
    OwnerTransferred,
    CallerMembershipRevoked,
    DaemonOffline,
    RemoteMachine,
    CliUnauthenticated,
    AgentArchived,
    AgentVersionChanged,
}

async fn database() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    SqliteDb::new(pool)
}

async fn create_user(db: &SqliteDb, id: &str, is_admin: bool, now: &str) {
    UserRepo::create_user(
        db,
        &User {
            id: id.to_owned(),
            email: format!("{id}@example.invalid"),
            password_hash: "unused-test-hash".to_owned(),
            display_name: None,
            is_admin,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .expect("user creates");
}

async fn seed_running_run() -> Fixture {
    let db = database().await;
    let now = now_rfc3339();
    let project_id = db::new_uuid_v4();
    let task_id = db::new_uuid_v4();
    let owner_id = db::new_uuid_v4();
    let caller_id = db::new_uuid_v4();
    let transfer_owner_id = db::new_uuid_v4();
    let agent_id = db::new_uuid_v4();
    let daemon_id = db::new_uuid_v4();
    let run_id = db::new_uuid_v4();

    create_user(&db, &owner_id, true, &now).await;
    create_user(&db, &caller_id, false, &now).await;
    create_user(&db, &transfer_owner_id, true, &now).await;

    DaemonRepo::upsert_by_machine_id(
        &db,
        UpsertDaemon {
            id: daemon_id.clone(),
            machine_id: format!("embedded:{daemon_id}"),
            hostname: "project-copilot-test-host".to_owned(),
            os: "linux".to_owned(),
            arch: "x86_64".to_owned(),
            agent_version: Some("fixture".to_owned()),
            labels_json: "{}".to_owned(),
            status: DaemonStatus::Online,
            registration_token_hash: Some("daemon-registration-hash".to_owned()),
            owner_id: Some(owner_id.clone()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("owned embedded daemon creates");
    DaemonRepo::update_report(
        &db,
        UpdateDaemonReport {
            id: daemon_id.clone(),
            last_report_at: now.clone(),
            status: DaemonStatus::Online,
            detected_clis_json: r#"[{"kind":"codex","availability":"authenticated"}]"#.to_owned(),
            labels_json: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("daemon reports authenticated CLI");

    let agent = AgentRepo::create(
        &db,
        CreateAgent {
            id: agent_id.clone(),
            name: "Project Copilot test Agent".to_owned(),
            description: None,
            executor_type: "codex".to_owned(),
            model: None,
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: Some(daemon_id.clone()),
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(owner_id.clone()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Agent creates with embedded host binding");

    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Project Copilot authority fixture".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(owner_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project creates with canonical owner membership");
    ProjectMemberRepo::add_member(
        &db,
        CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: project_id.clone(),
            user_id: caller_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("run caller joins Project");
    ProjectMemberRepo::add_member(
        &db,
        CreateProjectMember {
            id: db::new_uuid_v4(),
            project_id: project_id.clone(),
            user_id: transfer_owner_id.clone(),
            role: "admin".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("future owner joins Project");

    TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id: project_id.clone(),
            repo_id: None,
            parent_task_id: None,
            title: "Authority guarded WorkUnit".to_owned(),
            description: None,
            lifecycle_state: TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            subtask_order: None,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Task creates");
    TaskRoleRepo::create(
        &db,
        CreateTaskRole {
            id: db::new_uuid_v4(),
            task_id: task_id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("WorkUnit TaskRole creates");

    let binding = ProjectCopilotRepo::get_copilot_binding(&db, &project_id)
        .await
        .expect("default binding exists");
    let binding = ProjectCopilotRepo::update_copilot_binding(
        &db,
        &project_id,
        &owner_id,
        binding.version,
        Some(&agent_id),
        &now,
    )
    .await
    .expect("Project owner binds the Agent");
    let daemon = DaemonRepo::get_by_id(&db, &daemon_id)
        .await
        .expect("daemon loads")
        .expect("daemon exists");
    let run = CreateProjectCopilotRun {
        id: run_id.clone(),
        project_id: project_id.clone(),
        agent_id: agent_id.clone(),
        user_id: caller_id.clone(),
        binding_version: binding.version,
        agent_version: agent.version,
        agent_snapshot_json: serde_json::json!({
            "daemon_authority": db::daemon_authority_snapshot(&agent, &daemon)
        })
        .to_string(),
        context_task_id: Some(task_id.clone()),
        idempotency_key: format!("copilot-authority-{run_id}"),
        request_digest: "d".repeat(64),
        created_at: now.clone(),
    };
    let user_message = CreateProjectCopilotMessage {
        id: db::new_uuid_v4(),
        project_id: project_id.clone(),
        run_id: run_id.clone(),
        role: "user".to_owned(),
        actor_type: "human".to_owned(),
        actor_id: caller_id.clone(),
        content: "Create one scoped WorkUnit".to_owned(),
        created_at: now.clone(),
    };
    ProjectCopilotRepo::create_copilot_run(&db, run, user_message)
        .await
        .expect("run is admitted with current binding");
    ProjectCopilotRepo::claim_copilot_run(
        &db,
        &run_id,
        &"a".repeat(64),
        "2999-01-01T00:00:00Z",
        &now,
    )
    .await
    .expect("run claim succeeds")
    .expect("run enters running");
    ProjectCopilotRepo::authorize_run(&db, &run_id)
        .await
        .expect("seeded Project Copilot authority is live");

    Fixture {
        db,
        project_id,
        task_id,
        owner_id,
        caller_id,
        transfer_owner_id,
        agent_id,
        daemon_id,
        run_id,
    }
}

async fn insert_work_unit(
    tx: &mut Transaction<'_, Sqlite>,
    fixture: &Fixture,
    work_unit_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO work_unit (
            id, task_id, title, scope, role, created_by_kind, created_by_id,
            created_at, updated_at
         ) VALUES (?, ?, 'Copilot scoped change', 'Update the requested area',
                   'implementer', 'agent', ?, ?, ?)",
    )
    .bind(work_unit_id)
    .bind(&fixture.task_id)
    .bind(&fixture.agent_id)
    .bind(now_rfc3339())
    .bind(now_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_correlated_event(
    tx: &mut Transaction<'_, Sqlite>,
    fixture: &Fixture,
    work_unit_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO domain_event (
            id, event_type, entity_type, entity_id, actor_type, actor_id,
            scope_type, scope_id, correlation_id, causation_id, causation_depth,
            dedupe_key, payload_json, created_at
         ) VALUES (?, 'work_unit.created', 'work_unit', ?, 'agent', ?,
                   'task', ?, ?, NULL, 0, NULL, '{}', ?)",
    )
    .bind(db::new_uuid_v4())
    .bind(work_unit_id)
    .bind(&fixture.agent_id)
    .bind(&fixture.task_id)
    .bind(&fixture.run_id)
    .bind(now_rfc3339())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn assert_no_work_unit_or_event(fixture: &Fixture, work_unit_id: &str) {
    let work_unit_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_unit WHERE id = ?")
        .bind(work_unit_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("work-unit count query");
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE correlation_id = ? AND entity_id = ?",
    )
    .bind(&fixture.run_id)
    .bind(work_unit_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("event count query");
    assert_eq!(
        work_unit_count, 0,
        "failed authority must roll back WorkUnit"
    );
    assert_eq!(event_count, 0, "failed authority must not leave its event");
}

async fn change_authority(fixture: &Fixture, change: AuthorityChange) {
    let now = now_rfc3339();
    match change {
        AuthorityChange::OwnerTransferred => {
            let mut tx = fixture
                .db
                .pool()
                .begin_with("BEGIN IMMEDIATE")
                .await
                .expect("owner transfer transaction");
            sqlx::query(
                "UPDATE project_member SET role = 'member', updated_at = ?
                 WHERE project_id = ? AND user_id = ?",
            )
            .bind(&now)
            .bind(&fixture.project_id)
            .bind(&fixture.owner_id)
            .execute(&mut *tx)
            .await
            .expect("old owner membership demotes");
            sqlx::query(
                "UPDATE project_member SET role = 'owner', updated_at = ?
                 WHERE project_id = ? AND user_id = ?",
            )
            .bind(&now)
            .bind(&fixture.project_id)
            .bind(&fixture.transfer_owner_id)
            .execute(&mut *tx)
            .await
            .expect("new owner membership promotes");
            sqlx::query(
                "UPDATE project SET owner_id = ?, version = version + 1, updated_at = ?
                 WHERE id = ?",
            )
            .bind(&fixture.transfer_owner_id)
            .bind(&now)
            .bind(&fixture.project_id)
            .execute(&mut *tx)
            .await
            .expect("canonical Project owner transfers");
            tx.commit().await.expect("owner transfer commits");
        }
        AuthorityChange::CallerMembershipRevoked => {
            ProjectMemberRepo::remove_member(&fixture.db, &fixture.project_id, &fixture.caller_id)
                .await
                .expect("run caller membership revokes");
        }
        AuthorityChange::DaemonOffline => {
            DaemonRepo::mark_offline(&fixture.db, &fixture.daemon_id, &now)
                .await
                .expect("daemon goes offline");
        }
        AuthorityChange::RemoteMachine => {
            sqlx::query("UPDATE daemon SET machine_id = ? WHERE id = ?")
                .bind(format!("remote-host:{}", fixture.daemon_id))
                .bind(&fixture.daemon_id)
                .execute(fixture.db.pool())
                .await
                .expect("daemon changes to a nonembedded machine identity");
        }
        AuthorityChange::CliUnauthenticated => {
            DaemonRepo::update_report(
                &fixture.db,
                UpdateDaemonReport {
                    id: fixture.daemon_id.clone(),
                    last_report_at: now.clone(),
                    status: DaemonStatus::Online,
                    detected_clis_json: r#"[{"kind":"codex","availability":"missing"}]"#.to_owned(),
                    labels_json: None,
                    updated_at: now,
                },
            )
            .await
            .expect("daemon reports CLI as unauthenticated");
        }
        AuthorityChange::AgentArchived => {
            AgentRepo::archive(&fixture.db, &fixture.agent_id, &now)
                .await
                .expect("Agent archives");
        }
        AuthorityChange::AgentVersionChanged => {
            let agent = AgentRepo::get_by_id(&fixture.db, &fixture.agent_id)
                .await
                .expect("Agent loads")
                .expect("Agent exists");
            AgentRepo::update(
                &fixture.db,
                UpdateAgent {
                    id: fixture.agent_id.clone(),
                    expected_version: agent.version,
                    name: Some(format!("{} updated", agent.name)),
                    description: None,
                    model: None,
                    reasoning_effort: None,
                    permission_policy: None,
                    prompt_template: None,
                    capabilities_json: None,
                    config_json: None,
                    daemon_id: None,
                    max_concurrent_tasks: None,
                    heartbeat_interval_seconds: None,
                    max_missed_heartbeats: None,
                    status: None,
                    last_heartbeat_at: None,
                    is_default: None,
                    paused: None,
                    updated_at: now,
                },
            )
            .await
            .expect("Agent version increments");
        }
    }
}

async fn assert_event_rejected_after_change(change: AuthorityChange) {
    let fixture = seed_running_run().await;
    change_authority(&fixture, change).await;
    assert!(
        ProjectCopilotRepo::authorize_run(&fixture.db, &fixture.run_id)
            .await
            .is_err(),
        "{change:?} must revoke repository-level run authorization"
    );

    let work_unit_id = db::new_uuid_v4();
    let mut tx = fixture
        .db
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("mutation transaction");
    insert_work_unit(&mut tx, &fixture, &work_unit_id)
        .await
        .expect("tentative WorkUnit write succeeds before the guard");
    assert!(
        insert_correlated_event(&mut tx, &fixture, &work_unit_id)
            .await
            .is_err(),
        "{change:?} must reject the correlated domain_event"
    );
    tx.rollback().await.expect("rejected mutation rolls back");
    assert_no_work_unit_or_event(&fixture, &work_unit_id).await;
}

async fn seed_terminal_user_message(fixture: &Fixture, created_at: &str) -> String {
    let run_id = db::new_uuid_v4();
    let message_id = db::new_uuid_v4();
    sqlx::query(
        "INSERT INTO project_copilot_run (
            id, project_id, agent_id, user_id, binding_version, agent_version,
            agent_snapshot_json, context_task_id, status, idempotency_key,
            request_digest, created_at, updated_at
         )
         SELECT ?, project_id, agent_id, user_id, binding_version, agent_version,
                agent_snapshot_json, context_task_id, 'succeeded', ?, ?, ?, ?
         FROM project_copilot_run WHERE id = ?",
    )
    .bind(&run_id)
    .bind(format!("history-{run_id}"))
    .bind("e".repeat(64))
    .bind(created_at)
    .bind(created_at)
    .bind(&fixture.run_id)
    .execute(fixture.db.pool())
    .await
    .expect("historical terminal run creates");
    sqlx::query(
        "INSERT INTO project_copilot_message (
            id, project_id, run_id, role, actor_type, actor_id, content, created_at
         ) VALUES (?, ?, ?, 'user', 'human', ?, ?, ?)",
    )
    .bind(&message_id)
    .bind(&fixture.project_id)
    .bind(&run_id)
    .bind(&fixture.caller_id)
    .bind(format!("Historical message {created_at}"))
    .bind(created_at)
    .execute(fixture.db.pool())
    .await
    .expect("historical user message creates");
    message_id
}

#[tokio::test]
async fn project_copilot_domain_event_guard_allows_current_authority_and_rechecks_atomically() {
    let fixture = seed_running_run().await;
    let authorized = ProjectCopilotRepo::authorize_run(&fixture.db, &fixture.run_id)
        .await
        .expect("current live embedded Agent grant authorizes");
    assert_eq!(authorized.project_id, fixture.project_id);
    assert_eq!(authorized.agent_id, fixture.agent_id);
    assert_eq!(
        authorized.context_task_id.as_deref(),
        Some(fixture.task_id.as_str())
    );

    let committed_work_unit_id = db::new_uuid_v4();
    let mut tx = fixture
        .db
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("valid mutation transaction");
    insert_work_unit(&mut tx, &fixture, &committed_work_unit_id)
        .await
        .expect("valid WorkUnit write succeeds");
    insert_correlated_event(&mut tx, &fixture, &committed_work_unit_id)
        .await
        .expect("live Agent-correlated event passes the V139 guard");
    tx.commit().await.expect("valid mutation commits");

    let committed_work_unit_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_unit WHERE id = ?")
            .bind(&committed_work_unit_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("committed WorkUnit count");
    let committed_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE correlation_id = ? AND entity_id = ?",
    )
    .bind(&fixture.run_id)
    .bind(&committed_work_unit_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("committed event count");
    assert_eq!(committed_work_unit_count, 1);
    assert_eq!(committed_event_count, 1);

    let rolled_back_work_unit_id = db::new_uuid_v4();
    let mut tx = fixture
        .db
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .expect("revocation race transaction");
    insert_work_unit(&mut tx, &fixture, &rolled_back_work_unit_id)
        .await
        .expect("tentative WorkUnit is written first");
    sqlx::query(
        "UPDATE project_copilot SET version = version + 1, updated_at = ?
         WHERE project_id = ?",
    )
    .bind(now_rfc3339())
    .bind(&fixture.project_id)
    .execute(&mut *tx)
    .await
    .expect("binding version is revoked inside the mutation transaction");
    assert!(
        insert_correlated_event(&mut tx, &fixture, &rolled_back_work_unit_id)
            .await
            .is_err(),
        "the trigger sees revocation in the same transaction"
    );
    tx.rollback()
        .await
        .expect("rejected mutation and binding revocation roll back together");
    assert_no_work_unit_or_event(&fixture, &rolled_back_work_unit_id).await;
    ProjectCopilotRepo::authorize_run(&fixture.db, &fixture.run_id)
        .await
        .expect("rolling back the transaction restores the live grant");
}

#[tokio::test]
async fn project_copilot_domain_event_guard_rejects_changed_authority_edges() {
    for change in [
        AuthorityChange::OwnerTransferred,
        AuthorityChange::CallerMembershipRevoked,
        AuthorityChange::DaemonOffline,
        AuthorityChange::RemoteMachine,
        AuthorityChange::CliUnauthenticated,
        AuthorityChange::AgentArchived,
        AuthorityChange::AgentVersionChanged,
    ] {
        assert_event_rejected_after_change(change).await;
    }
}

#[tokio::test]
async fn project_copilot_message_pages_start_latest_and_walk_older_then_refresh_latest_reply() {
    let fixture = seed_running_run().await;
    let first_at = "2026-01-01T00:00:01Z";
    let second_at = "2026-01-01T00:00:02Z";
    let third_at = "2026-01-01T00:00:03Z";
    let assistant_at = "2026-01-01T00:00:04Z";
    let original_user_message_id: String = sqlx::query_scalar(
        "SELECT id FROM project_copilot_message WHERE run_id = ? AND role = 'user'",
    )
    .bind(&fixture.run_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("fixture user message exists");
    sqlx::query("UPDATE project_copilot_message SET created_at = ? WHERE id = ?")
        .bind(first_at)
        .bind(&original_user_message_id)
        .execute(fixture.db.pool())
        .await
        .expect("oldest message gets a deterministic timestamp");
    let second_message_id = seed_terminal_user_message(&fixture, second_at).await;
    let third_message_id = seed_terminal_user_message(&fixture, third_at).await;

    let first_page = ProjectCopilotRepo::list_copilot_messages(
        &fixture.db,
        &fixture.project_id,
        CopilotMessagePageQuery {
            cursor: None,
            limit: 2,
        },
    )
    .await
    .expect("initial message page loads");
    assert_eq!(
        first_page
            .items
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec![second_message_id.as_str(), third_message_id.as_str()],
        "first page contains the latest messages in chronological display order"
    );
    let older_cursor = first_page
        .next_cursor
        .as_deref()
        .expect("older history has a cursor")
        .to_owned();
    let older_page = ProjectCopilotRepo::list_copilot_messages(
        &fixture.db,
        &fixture.project_id,
        CopilotMessagePageQuery {
            cursor: Some(older_cursor),
            limit: 2,
        },
    )
    .await
    .expect("older message page loads");
    assert_eq!(older_page.items.len(), 1);
    assert_eq!(older_page.items[0].id, original_user_message_id);
    assert!(older_page.next_cursor.is_none());

    let assistant_message_id = db::new_uuid_v4();
    ProjectCopilotRepo::finish_copilot_run(
        &fixture.db,
        &fixture.run_id,
        ProjectCopilotRunStatus::Succeeded,
        Some(db::CreateProjectCopilotMessage {
            id: assistant_message_id.clone(),
            project_id: fixture.project_id.clone(),
            run_id: fixture.run_id.clone(),
            role: "assistant".to_owned(),
            actor_type: "agent".to_owned(),
            actor_id: fixture.agent_id.clone(),
            content: "The delegated planning response is ready.".to_owned(),
            created_at: assistant_at.to_owned(),
        }),
        None,
        None,
        &now_rfc3339(),
    )
    .await
    .expect("run completes with terminal assistant reply")
    .expect("run exists");

    let refreshed = ProjectCopilotRepo::list_copilot_messages(
        &fixture.db,
        &fixture.project_id,
        CopilotMessagePageQuery {
            cursor: None,
            limit: 2,
        },
    )
    .await
    .expect("refreshed initial message page loads");
    assert_eq!(refreshed.items.len(), 2);
    assert_eq!(refreshed.items[0].id, third_message_id);
    assert_eq!(refreshed.items[1].id, assistant_message_id);
    assert_eq!(refreshed.items[1].role, "assistant");
    assert_eq!(
        refreshed.items[1].content,
        "The delegated planning response is ready."
    );
}

impl std::fmt::Debug for AuthorityChange {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::OwnerTransferred => "OwnerTransferred",
            Self::CallerMembershipRevoked => "CallerMembershipRevoked",
            Self::DaemonOffline => "DaemonOffline",
            Self::RemoteMachine => "RemoteMachine",
            Self::CliUnauthenticated => "CliUnauthenticated",
            Self::AgentArchived => "AgentArchived",
            Self::AgentVersionChanged => "AgentVersionChanged",
        };
        formatter.write_str(name)
    }
}
