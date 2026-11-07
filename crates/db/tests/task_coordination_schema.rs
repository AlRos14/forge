use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AuthorizeTaskCoordinationInput,
    CreateDomainEvent, CreateProject, CreateRepo, CreateTask, ProjectRepo, RepoRepo, SqliteDb,
    TaskCoordinationMode, TaskCoordinationOperation, TaskCoordinationRepo, TaskRepo, WorkMode,
};
use sha2::Digest;

async fn database() -> SqliteDb {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    SqliteDb::new(pool)
}

fn event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    user_id: &str,
) -> CreateDomainEvent {
    let now = now_rfc3339();
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(user_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: new_uuid_v4(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: serde_json::json!({"task_id": task_id}).to_string(),
        created_at: now,
    }
}

async fn seed_task(db: &SqliteDb) -> (String, String) {
    let now = now_rfc3339();
    let user_id = new_uuid_v4();
    db::UserRepo::create_user(
        db,
        &db::User {
            id: user_id.clone(),
            email: format!("{user_id}@example.invalid"),
            password_hash: "unused".to_owned(),
            display_name: Some("Owner".to_owned()),
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("user");
    let project_id = new_uuid_v4();
    ProjectRepo::create(
        db,
        CreateProject {
            id: project_id.clone(),
            name: "coordination".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(user_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project");
    let repo_id = new_uuid_v4();
    RepoRepo::create(
        db,
        CreateRepo {
            id: repo_id.clone(),
            project_id,
            name: "repo".to_owned(),
            remote_url: "https://example.invalid/repo.git".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("repo");
    let task_id = new_uuid_v4();
    TaskRepo::create(
        db,
        CreateTask {
            id: task_id.clone(),
            project_id: db::RepoRepo::get_by_id(db, &repo_id)
                .await
                .expect("repo")
                .expect("repo row")
                .project_id,
            repo_id: Some(repo_id),
            parent_task_id: None,
            subtask_order: None,
            title: "Coordination task".to_owned(),
            description: Some("objective".to_owned()),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("task");
    (task_id, user_id)
}

#[tokio::test]
async fn migration_backfills_manual_and_creates_new_task_rows() {
    let db = database().await;
    let (task_id, _) = seed_task(&db).await;
    let coordination = TaskCoordinationRepo::get_task_coordination(&db, &task_id)
        .await
        .expect("read coordination")
        .expect("backfilled coordination");
    assert_eq!(coordination.mode, TaskCoordinationMode::Manual);
    assert_eq!(coordination.authorization_version, 1);
    assert_eq!(coordination.turn_count, 0);
    assert_eq!(coordination.max_turns, 8);
    assert_eq!(coordination.max_quality_requests, 8);
    assert!(coordination.coordinator_actor.is_none());
}

#[tokio::test]
async fn authorization_receipts_are_idempotent_and_fence_conflicts() {
    let db = database().await;
    let (task_id, user_id) = seed_task(&db).await;
    let now = now_rfc3339();
    let team_snapshot_json = r#"{"memberships":[],"schema_version":1,"task_roles":[]}"#.to_owned();
    let team_digest = hex::encode(sha2::Sha256::digest(team_snapshot_json.as_bytes()));
    let envelope_json = "{}".to_owned();
    let envelope_digest = hex::encode(sha2::Sha256::digest(envelope_json.as_bytes()));
    let request_digest = hex::encode(sha2::Sha256::digest(b"authorize"));
    let input = |receipt_id: String, digest: String| AuthorizeTaskCoordinationInput {
        task_id: task_id.clone(),
        expected_coordination_version: 1,
        mode: TaskCoordinationMode::Manual,
        coordinator_task_role_id: None,
        coordinator_actor: None,
        team_snapshot_json: team_snapshot_json.clone(),
        team_digest: team_digest.clone(),
        envelope_json: envelope_json.clone(),
        envelope_digest: envelope_digest.clone(),
        requested_by: user_id.clone(),
        idempotency_key: "authorize-1".to_owned(),
        request_digest: digest,
        receipt_id,
        created_at: now.clone(),
        event: event(
            "task.coordination_authorized",
            "task_coordination",
            &task_id,
            &task_id,
            &user_id,
        ),
    };
    let first = TaskCoordinationRepo::authorize_task_coordination(
        &db,
        input(new_uuid_v4(), request_digest.clone()),
        1,
    )
    .await
    .expect("authorize");
    assert!(!first.replayed);
    assert_eq!(first.coordination.authorization_version, 2);

    let replay = TaskCoordinationRepo::authorize_task_coordination(
        &db,
        input(new_uuid_v4(), request_digest),
        1,
    )
    .await
    .expect("replay");
    assert!(replay.replayed);
    assert_eq!(replay.coordination.authorization_version, 2);
    assert_eq!(
        replay.receipt.operation,
        TaskCoordinationOperation::Authorize
    );

    let conflict = TaskCoordinationRepo::authorize_task_coordination(
        &db,
        input(
            new_uuid_v4(),
            hex::encode(sha2::Sha256::digest(b"different")),
        ),
        1,
    )
    .await
    .expect_err("same key with different content must conflict");
    assert!(matches!(conflict, db::DbError::IdempotencyConflict));
}

#[tokio::test]
async fn turn_budget_is_global_and_does_not_reset() {
    let db = database().await;
    let (task_id, _) = seed_task(&db).await;
    for _ in 0..8 {
        assert!(
            TaskCoordinationRepo::consume_coordination_turn_budget(&db, &task_id)
                .await
                .expect("consume")
        );
    }
    assert!(
        !TaskCoordinationRepo::consume_coordination_turn_budget(&db, &task_id)
            .await
            .expect("exhausted")
    );
    TaskCoordinationRepo::refund_coordination_turn_budget(&db, &task_id)
        .await
        .expect("refund");
    assert!(
        TaskCoordinationRepo::consume_coordination_turn_budget(&db, &task_id)
            .await
            .expect("one refunded turn")
    );
}
