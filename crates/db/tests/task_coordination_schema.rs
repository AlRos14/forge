use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AuthorizeTaskCoordinationInput,
    CreateDomainEvent, CreateProject, CreateRepo, CreateTask, ProjectRepo, RepoRepo,
    RoleMembershipRepo, SqliteDb, TaskCoordinationMode, TaskCoordinationOperation,
    TaskCoordinationRepo, TaskRepo, TaskRepositorySetRepo, WorkMode, WorkUnitRepo,
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
            repositories: Vec::new(),
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
    assert_eq!(coordination.max_turns, 32);
    assert_eq!(coordination.max_quality_requests, 64);
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
        expected_team_generation: 0,
        mode: TaskCoordinationMode::Manual,
        coordinator_task_role_id: None,
        coordinator_actor: None,
        team_snapshot_json: team_snapshot_json.clone(),
        team_digest: team_digest.clone(),
        envelope_json: envelope_json.clone(),
        envelope_digest: envelope_digest.clone(),
        unconfigured_default_refresh: false,
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
    for _ in 0..32 {
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

#[tokio::test]
async fn materialization_is_atomic_and_review_subject_is_exact() {
    let db = database().await;
    let (task_id, owner_id) = seed_task(&db).await;
    let task = TaskRepo::get_by_id(&db, &task_id, false)
        .await
        .expect("task")
        .expect("task row");
    let now = now_rfc3339();
    let reviewer_id = new_uuid_v4();
    db::UserRepo::create_user(
        &db,
        &db::User {
            id: reviewer_id.clone(),
            email: format!("{reviewer_id}@example.invalid"),
            password_hash: "unused".to_owned(),
            display_name: Some("Reviewer".to_owned()),
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("reviewer user");
    let implementer_role_id = new_uuid_v4();
    db::TaskRoleRepo::create(
        &db,
        db::CreateTaskRole {
            id: implementer_role_id.clone(),
            task_id: task_id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: Some(db::CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("implementer role");
    let reviewer_role_id = new_uuid_v4();
    db::TaskRoleRepo::create_with_configuration(
        &db,
        db::CreateTaskRole {
            id: reviewer_role_id.clone(),
            task_id: task_id.clone(),
            role: "reviewer".to_owned(),
            coordination_mode: Some(db::CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":"Reviewer","capabilities":["formal_review"],"rework":null}"#,
    )
    .await
    .expect("reviewer role");
    for (role_id, actor_id) in [
        (implementer_role_id.clone(), owner_id.clone()),
        (reviewer_role_id.clone(), reviewer_id.clone()),
    ] {
        RoleMembershipRepo::add(
            &db,
            db::CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: role_id,
                actor_kind: db::ActorKind::Human,
                actor_id,
                status: db::RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("membership");
    }

    let team_snapshot_json = r#"{"memberships":[],"schema_version":1,"task_roles":[]}"#.to_owned();
    let team_digest = hex::encode(sha2::Sha256::digest(team_snapshot_json.as_bytes()));
    let envelope_json = "{}".to_owned();
    let envelope_digest = hex::encode(sha2::Sha256::digest(envelope_json.as_bytes()));
    let team_generation = TaskCoordinationRepo::get_task_coordination(&db, &task_id)
        .await
        .expect("coordination")
        .expect("coordination row")
        .team_generation;
    TaskCoordinationRepo::authorize_task_coordination(
        &db,
        AuthorizeTaskCoordinationInput {
            task_id: task_id.clone(),
            expected_coordination_version: 1,
            expected_team_generation: team_generation,
            mode: TaskCoordinationMode::PrepareAndExecute,
            coordinator_task_role_id: Some(implementer_role_id.clone()),
            coordinator_actor: Some(db::ActorRef::Human(owner_id.clone())),
            team_snapshot_json,
            team_digest: team_digest.clone(),
            envelope_json,
            envelope_digest,
            unconfigured_default_refresh: false,
            requested_by: owner_id.clone(),
            idempotency_key: "materialize-test-authorize".to_owned(),
            request_digest: hex::encode(sha2::Sha256::digest(b"authorize-materialize")),
            receipt_id: new_uuid_v4(),
            created_at: now.clone(),
            event: event(
                "task.coordination_authorized",
                "task_coordination",
                &task_id,
                &task_id,
                &owner_id,
            ),
        },
        1,
    )
    .await
    .expect("authorize coordination");

    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
            id, task_id, agent_id, role, status, summary, created_at, updated_at,
            actor_kind, actor_id
         ) VALUES (?, ?, NULL, 'implementer', 'completed', '{}', ?, ?, 'human', ?)",
    )
    .bind(&execution_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .bind(&owner_id)
    .execute(db.pool())
    .await
    .expect("producer execution");
    let artifact_id = new_uuid_v4();
    let plan_digest = hex::encode(sha2::Sha256::digest(b"{}"));
    let mut artifact_tx = db.pool().begin().await.expect("artifact transaction");
    sqlx::query("INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id) VALUES (?, ?, ?)")
        .bind(&artifact_id)
        .bind(&execution_id)
        .bind(&task_id)
        .execute(&mut *artifact_tx)
        .await
        .expect("artifact producer");
    sqlx::query(
        "INSERT INTO artifact (id, task_id, kind, storage_kind, content, content_ref, metadata_json, digest, created_at)
         VALUES (?, ?, 'work_unit_plan', 'inline', '{}', NULL, '{}', ?, ?)",
    )
    .bind(&artifact_id)
    .bind(&task_id)
    .bind(&plan_digest)
    .bind(&now)
    .execute(&mut *artifact_tx)
    .await
    .expect("plan artifact");
    artifact_tx.commit().await.expect("artifact commit");
    sqlx::query(
        "INSERT INTO execution_artifact_output (execution_id, artifact_id, task_id, kind, digest, created_at)
         VALUES (?, ?, ?, 'work_unit_plan', ?, ?)",
    )
    .bind(&execution_id)
    .bind(&artifact_id)
    .bind(&task_id)
    .bind(&plan_digest)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("artifact output");
    let request_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json, objective_digest,
            request_digest, idempotency_key, state, plan_artifact_id, version,
            created_at, updated_at
         ) VALUES (?, ?, 2, 'prepare_and_execute', '{}', ?, ?, 'materialize-test', 'plan_ready', ?, 1, ?, ?)",
    )
    .bind(&request_id)
    .bind(&task_id)
    .bind(hex::encode(sha2::Sha256::digest(b"brief")))
    .bind(hex::encode(sha2::Sha256::digest(b"request")))
    .bind(&artifact_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("coordination request");
    sqlx::query(
        "INSERT INTO task_coordination_plan (
            artifact_id, task_id, request_id, authorization_version, plan_digest,
            state, version, created_at, updated_at
         ) VALUES (?, ?, ?, 2, ?, 'approved', 1, ?, ?)",
    )
    .bind(&artifact_id)
    .bind(&task_id)
    .bind(&request_id)
    .bind(&plan_digest)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("plan row");

    let implementer_work_unit_id = new_uuid_v4();
    let reviewer_work_unit_id = new_uuid_v4();
    let consumer_work_unit_id = new_uuid_v4();
    let second_reviewer_work_unit_id = new_uuid_v4();
    let pending_parent_work_unit_id = new_uuid_v4();
    let pending_child_work_unit_id = new_uuid_v4();
    let repository_set = TaskRepositorySetRepo::get_current(&db, &task_id)
        .await
        .expect("repository-set lookup")
        .expect("current repository set");
    let materialization_input = db::MaterializeTaskCoordinationPlanInput {
        receipt_id: new_uuid_v4(),
        task_id: task_id.clone(),
        request_id: request_id.clone(),
        plan_artifact_id: artifact_id.clone(),
        plan_digest: plan_digest.clone(),
        authorization_version: 2,
        expected_team_digest: team_digest.clone(),
        expected_plan_version: 1,
        task_version_before: task.version,
        expected_request_version: 1,
        expected_repository_set_revision: repository_set.revision,
        expected_repository_set_digest: repository_set.digest.clone(),
        repository_set_revision_change: None,
        coordinator_replan_decision: None,
        work_units: vec![
            db::MaterializedWorkUnit {
                id: pending_parent_work_unit_id.clone(),
                key: "pending-parent".to_owned(),
                title: "Parent waits for review".to_owned(),
                scope: "Parent after review approval".to_owned(),
                execution_instructions: None,
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(owner_id.clone())),
                parent_work_unit_id: None,
                depends_on_work_unit_ids: vec![reviewer_work_unit_id.clone()],
                requires_integration: false,
                artifact_inputs: vec![],
                review_subject_work_unit_id: None,
            },
            db::MaterializedWorkUnit {
                id: pending_child_work_unit_id.clone(),
                key: "pending-child".to_owned(),
                title: "Child of deferred parent".to_owned(),
                scope: "Child after parent".to_owned(),
                execution_instructions: None,
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(owner_id.clone())),
                parent_work_unit_id: Some(pending_parent_work_unit_id.clone()),
                depends_on_work_unit_ids: vec![],
                requires_integration: false,
                artifact_inputs: vec![],
                review_subject_work_unit_id: None,
            },
            db::MaterializedWorkUnit {
                id: implementer_work_unit_id.clone(),
                key: "implementation".to_owned(),
                title: "Implement".to_owned(),
                scope: "Deliver the change".to_owned(),
                execution_instructions: None,
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(owner_id.clone())),
                parent_work_unit_id: None,
                depends_on_work_unit_ids: vec![],
                requires_integration: false,
                artifact_inputs: vec![],
                review_subject_work_unit_id: None,
            },
            db::MaterializedWorkUnit {
                id: reviewer_work_unit_id.clone(),
                key: "review".to_owned(),
                title: "Review".to_owned(),
                scope: "Review the exact delivery".to_owned(),
                execution_instructions: None,
                role: "reviewer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(reviewer_id.clone())),
                parent_work_unit_id: None,
                depends_on_work_unit_ids: vec![implementer_work_unit_id.clone()],
                requires_integration: false,
                artifact_inputs: vec![],
                review_subject_work_unit_id: Some(implementer_work_unit_id.clone()),
            },
            db::MaterializedWorkUnit {
                id: consumer_work_unit_id.clone(),
                key: "consumer".to_owned(),
                title: "Second implementation".to_owned(),
                scope: "Proceed only after review approval".to_owned(),
                execution_instructions: None,
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(owner_id.clone())),
                parent_work_unit_id: None,
                depends_on_work_unit_ids: vec![reviewer_work_unit_id.clone()],
                requires_integration: false,
                artifact_inputs: vec![],
                review_subject_work_unit_id: None,
            },
            db::MaterializedWorkUnit {
                id: second_reviewer_work_unit_id.clone(),
                key: "second-review".to_owned(),
                title: "Review second implementation".to_owned(),
                scope: "Review the approved second delivery".to_owned(),
                execution_instructions: None,
                role: "reviewer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(reviewer_id.clone())),
                parent_work_unit_id: None,
                depends_on_work_unit_ids: vec![consumer_work_unit_id.clone()],
                requires_integration: false,
                artifact_inputs: vec![],
                review_subject_work_unit_id: Some(consumer_work_unit_id.clone()),
            },
        ],
        receipt_digest: hex::encode(sha2::Sha256::digest(b"receipt")),
        created_by: owner_id.clone(),
        created_at: now.clone(),
        events: vec![],
    };
    sqlx::query("UPDATE task_coordination SET team_stale = 1 WHERE task_id = ?")
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("mark stale");
    let stale = TaskCoordinationRepo::materialize_task_coordination_plan(
        &db,
        materialization_input.clone(),
    )
    .await
    .expect_err("a stale team must block materialization inside the transaction");
    assert!(matches!(stale, db::DbError::VersionConflict));
    sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("clear stale marker");
    let receipt =
        TaskCoordinationRepo::materialize_task_coordination_plan(&db, materialization_input)
            .await
            .expect("atomic materialization");
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&receipt.work_unit_ids_json)
            .expect("ids")
            .len(),
        6
    );
    let units = WorkUnitRepo::list_by_task(&db, &task_id)
        .await
        .expect("work units");
    assert_eq!(units.len(), 6);
    let subject: (String, Option<i64>) = sqlx::query_as(
        "SELECT reviewed_work_unit_id, reviewed_work_unit_version
         FROM work_unit_review_subject WHERE review_work_unit_id = ?",
    )
    .bind(&reviewer_work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("review subject");
    assert_eq!(subject.0, implementer_work_unit_id);
    assert_eq!(subject.1, None);
    let second_subject: String = sqlx::query_scalar(
        "SELECT reviewed_work_unit_id FROM work_unit_review_subject
         WHERE review_work_unit_id = ?",
    )
    .bind(&second_reviewer_work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("second review subject");
    assert_eq!(second_subject, consumer_work_unit_id);
    let line: (String, i64) = sqlx::query_as(
        "SELECT id, revision FROM work_unit_review_line
         WHERE task_id = ? AND root_work_unit_id = ?",
    )
    .bind(&task_id)
    .bind(&implementer_work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("review line");
    assert_eq!(line.1, 1);
    let gate: (String, Option<i64>) = sqlx::query_as(
        "SELECT id, active_policy_revision FROM gate
         WHERE task_id = ? AND gate_kind LIKE 'review_approval:%'
           AND scope_kind = 'work_unit' AND scope_id = ?",
    )
    .bind(&task_id)
    .bind(&implementer_work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("review approval gate");
    assert_eq!(gate.1, Some(1));
    let consumer = units
        .iter()
        .find(|unit| unit.id == consumer_work_unit_id)
        .expect("consumer");
    assert_eq!(
        consumer.review_approval_gate_id.as_deref(),
        Some(gate.0.as_str())
    );
    assert_eq!(
        consumer.review_approval_line_id.as_deref(),
        Some(line.0.as_str())
    );
    let parent = units
        .iter()
        .find(|unit| unit.id == pending_parent_work_unit_id)
        .expect("pending parent");
    assert_eq!(
        parent.review_approval_gate_id.as_deref(),
        Some(gate.0.as_str())
    );
    let child = units
        .iter()
        .find(|unit| unit.id == pending_child_work_unit_id)
        .expect("pending child");
    assert_eq!(
        child.parent_work_unit_id.as_deref(),
        Some(pending_parent_work_unit_id.as_str())
    );
    let replay = TaskCoordinationRepo::materialize_task_coordination_plan(
        &db,
        db::MaterializeTaskCoordinationPlanInput {
            receipt_id: new_uuid_v4(),
            task_id: task_id.clone(),
            request_id,
            plan_artifact_id: artifact_id,
            plan_digest,
            authorization_version: 2,
            expected_team_digest: team_digest,
            expected_plan_version: 1,
            task_version_before: task.version,
            expected_request_version: 1,
            expected_repository_set_revision: repository_set.revision,
            expected_repository_set_digest: repository_set.digest.clone(),
            repository_set_revision_change: None,
            coordinator_replan_decision: None,
            work_units: vec![],
            receipt_digest: hex::encode(sha2::Sha256::digest(b"other")),
            created_by: owner_id.clone(),
            created_at: now,
            events: vec![],
        },
    )
    .await
    .expect("replay receipt");
    assert_eq!(replay.id, receipt.id);

    let consumer = WorkUnitRepo::get_by_id(&db, &consumer_work_unit_id)
        .await
        .expect("consumer")
        .expect("consumer row");
    assert_eq!(consumer.status, db::WorkUnitStatus::Open);
    let fence_input = || db::TransitionWorkUnit {
        id: consumer_work_unit_id.clone(),
        expected_version: consumer.version,
        status: db::WorkUnitStatus::Completed,
        updated_at: now_rfc3339(),
        reviewed_result: None,
    };
    sqlx::query("UPDATE task_coordination SET revoked_at = ? WHERE task_id = ?")
        .bind(now_rfc3339())
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("revoke coordination");
    let revoked = WorkUnitRepo::transition_with_coordination_fence(
        &db,
        fence_input(),
        2,
        event(
            "work_unit.completed",
            "work_unit",
            &consumer_work_unit_id,
            &task_id,
            &owner_id,
        ),
    )
    .await
    .expect_err("revoked authorization must fence completion");
    assert!(matches!(revoked, db::DbError::VersionConflict));
    sqlx::query(
        "UPDATE task_coordination SET revoked_at = NULL, mode = 'manual' WHERE task_id = ?",
    )
    .bind(&task_id)
    .execute(db.pool())
    .await
    .expect("manual coordination");
    let manual = WorkUnitRepo::transition_with_coordination_fence(
        &db,
        fence_input(),
        2,
        event(
            "work_unit.completed",
            "work_unit",
            &consumer_work_unit_id,
            &task_id,
            &owner_id,
        ),
    )
    .await
    .expect_err("manual mode must fence completion");
    assert!(matches!(manual, db::DbError::VersionConflict));
    sqlx::query("UPDATE task_coordination SET mode = 'proposal', team_stale = 1 WHERE task_id = ?")
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("stale team");
    let stale = WorkUnitRepo::transition_with_coordination_fence(
        &db,
        fence_input(),
        2,
        event(
            "work_unit.completed",
            "work_unit",
            &consumer_work_unit_id,
            &task_id,
            &owner_id,
        ),
    )
    .await
    .expect_err("stale team must fence completion");
    assert!(matches!(stale, db::DbError::VersionConflict));
    let still_open = WorkUnitRepo::get_by_id(&db, &consumer_work_unit_id)
        .await
        .expect("consumer")
        .expect("consumer row");
    assert_eq!(still_open.status, db::WorkUnitStatus::Open);
}

#[tokio::test]
async fn multirepo_lifecycle_replan_approved_resume_materializes_revision_and_preserves_old_pins() {
    let db = database().await;
    let (task_id, owner_id) = seed_task(&db).await;
    let task = TaskRepo::get_by_id(&db, &task_id, false)
        .await
        .expect("Task loads")
        .expect("Task exists");
    let now = now_rfc3339();
    let current = TaskRepositorySetRepo::get_current(&db, &task_id)
        .await
        .expect("repository revision loads")
        .expect("Task has a primary repository");
    let primary_repo_id = current.repositories[0].repo_id.clone();
    let added_repo_id = new_uuid_v4();
    RepoRepo::create(
        &db,
        CreateRepo {
            id: added_repo_id.clone(),
            project_id: task.project_id.clone(),
            name: "replan-added".to_owned(),
            remote_url: "https://example.invalid/replan-added.git".to_owned(),
            local_path: None,
            work_mode: WorkMode::DirectMerge,
            default_branch: "main".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("added repository belongs to the Task Project");

    let role_id = new_uuid_v4();
    db::TaskRoleRepo::create(
        &db,
        db::CreateTaskRole {
            id: role_id,
            task_id: task_id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: Some(db::CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("implementer role creates");
    db::RoleMembershipRepo::add(
        &db,
        db::CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: db::TaskRoleRepo::get_by_task_and_role(&db, &task_id, "implementer")
                .await
                .expect("role lookup")
                .expect("implementer role")
                .id,
            actor_kind: db::ActorKind::Human,
            actor_id: owner_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("owner is an implementer");

    let team_snapshot_json = r#"{"memberships":[],"schema_version":1,"task_roles":[]}"#.to_owned();
    let team_digest = hex::encode(sha2::Sha256::digest(team_snapshot_json.as_bytes()));
    let envelope_json = "{}".to_owned();
    let envelope_digest = hex::encode(sha2::Sha256::digest(envelope_json.as_bytes()));
    let coordination = TaskCoordinationRepo::get_task_coordination(&db, &task_id)
        .await
        .expect("coordination loads")
        .expect("coordination exists");
    TaskCoordinationRepo::authorize_task_coordination(
        &db,
        AuthorizeTaskCoordinationInput {
            task_id: task_id.clone(),
            expected_coordination_version: coordination.authorization_version,
            expected_team_generation: coordination.team_generation,
            mode: TaskCoordinationMode::PrepareAndExecute,
            coordinator_task_role_id: Some(
                db::TaskRoleRepo::get_by_task_and_role(&db, &task_id, "implementer")
                    .await
                    .expect("role lookup")
                    .expect("implementer role")
                    .id,
            ),
            coordinator_actor: Some(db::ActorRef::Human(owner_id.clone())),
            team_snapshot_json,
            team_digest: team_digest.clone(),
            envelope_json,
            envelope_digest,
            unconfigured_default_refresh: false,
            requested_by: owner_id.clone(),
            idempotency_key: "replan-materialize-authorize".to_owned(),
            request_digest: hex::encode(sha2::Sha256::digest(b"replan-authorize")),
            receipt_id: new_uuid_v4(),
            created_at: now.clone(),
            event: event(
                "task.coordination_authorized",
                "task_coordination",
                &task_id,
                &task_id,
                &owner_id,
            ),
        },
        coordination.authorization_version,
    )
    .await
    .expect("coordination authorizes");

    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
            id, task_id, agent_id, role, status, summary, created_at, updated_at,
            actor_kind, actor_id
         ) VALUES (?, ?, NULL, 'implementer', 'completed', '{}', ?, ?, 'human', ?)",
    )
    .bind(&execution_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .bind(&owner_id)
    .execute(db.pool())
    .await
    .expect("replan producer Execution inserts");
    let artifact_id = new_uuid_v4();
    let plan_digest = hex::encode(sha2::Sha256::digest(b"{}"));
    let mut artifact_tx = db
        .pool()
        .begin()
        .await
        .expect("Artifact transaction begins");
    sqlx::query(
        "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
         VALUES (?, ?, ?)",
    )
    .bind(&artifact_id)
    .bind(&execution_id)
    .bind(&task_id)
    .execute(&mut *artifact_tx)
    .await
    .expect("Artifact producer binds");
    sqlx::query(
        "INSERT INTO artifact (
            id, task_id, kind, storage_kind, content, content_ref, metadata_json, digest, created_at
         ) VALUES (?, ?, 'work_unit_plan', 'inline', '{}', NULL, '{}', ?, ?)",
    )
    .bind(&artifact_id)
    .bind(&task_id)
    .bind(&plan_digest)
    .bind(&now)
    .execute(&mut *artifact_tx)
    .await
    .expect("plan Artifact inserts");
    artifact_tx
        .commit()
        .await
        .expect("Artifact transaction commits");
    sqlx::query(
        "INSERT INTO execution_artifact_output (
            execution_id, artifact_id, task_id, kind, digest, created_at
         ) VALUES (?, ?, ?, 'work_unit_plan', ?, ?)",
    )
    .bind(&execution_id)
    .bind(&artifact_id)
    .bind(&task_id)
    .bind(&plan_digest)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("Execution publishes plan Artifact");

    let request_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json, objective_digest,
            request_digest, idempotency_key, state, plan_artifact_id, version,
            created_at, updated_at
         ) VALUES (?, ?, ?, 'prepare_and_execute', '{}', ?, ?, 'replan-plan',
                   'plan_ready', ?, 1, ?, ?)",
    )
    .bind(&request_id)
    .bind(&task_id)
    .bind(coordination.authorization_version + 1)
    .bind(hex::encode(sha2::Sha256::digest(b"brief")))
    .bind(hex::encode(sha2::Sha256::digest(b"request")))
    .bind(&artifact_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("replan plan request inserts");
    sqlx::query(
        "INSERT INTO task_coordination_plan (
            artifact_id, task_id, request_id, authorization_version, plan_digest,
            state, version, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, 'approved', 1, ?, ?)",
    )
    .bind(&artifact_id)
    .bind(&task_id)
    .bind(&request_id)
    .bind(coordination.authorization_version + 1)
    .bind(&plan_digest)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("approved plan row inserts");

    let replan_id = new_uuid_v4();
    db::TaskCoordinatorReplanRepo::create_coordinator_replan(
        &db,
        db::CreateCoordinatorReplan {
            id: replan_id.clone(),
            task_id: task_id.clone(),
            authorization_version: coordination.authorization_version + 1,
            source_request_id: request_id.clone(),
            producer_execution_id: execution_id.clone(),
            reason: "add a repository".to_owned(),
            evidence_links_json: "[]".to_owned(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("coordinator replan creates");
    db::TaskCoordinatorReplanRepo::transition_coordinator_replan(
        &db,
        db::TransitionCoordinatorReplan {
            id: replan_id.clone(),
            expected_version: 1,
            to_state: db::CoordinatorReplanState::Planning,
            plan_request_id: None,
            classification: None,
            classification_facts_json: None,
            human_decision_id: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("replan enters planning");

    let decision_id = new_uuid_v4();
    let binding = serde_json::json!({
        "request_id": request_id,
        "request_kind": "work_unit_plan",
        "authorization_version": coordination.authorization_version + 1,
        "source_subject": {
            "coordinator_replan_id": replan_id,
            "plan_request_id": request_id,
            "plan_artifact_id": artifact_id,
            "plan_digest": plan_digest,
        }
    });
    db::HumanDecisionRepo::create_human_decision_request(
        &db,
        db::CreateHumanDecisionRequest {
            id: decision_id.clone(),
            task_id: task_id.clone(),
            source_kind: db::HumanDecisionSourceKind::CoordinatorReplan,
            source_id: replan_id.clone(),
            raised_by_execution_id: Some(execution_id),
            authorization_version: Some(coordination.authorization_version + 1),
            question: "Approve the repository-set replan?".to_owned(),
            options_json: r#"[
                {"id":"approve_replan","label":"Approve","description":"Approve exact plan","consequence":"Materialize exact plan","authority_effect":"scope"},
                {"id":"reject","label":"Reject","description":"Reject plan","consequence":"Do not materialize plan","authority_effect":"none"}
            ]"#
            .to_owned(),
            recommended_option_id: None,
            recommendation_rationale: None,
            evidence_links_json: format!(
                "[{{\"kind\":\"artifact\",\"id\":\"{artifact_id}\",\"digest\":\"{plan_digest}\"}}]"
            ),
            max_authority_effect: db::AuthorityEffect::Scope,
            resume_binding_json: binding.to_string(),
            created_at: now.clone(),
        },
        None,
    )
    .await
    .expect("exact Human decision opens");
    db::HumanDecisionRepo::answer_human_decision(
        &db,
        db::AnswerHumanDecision {
            id: decision_id.clone(),
            expected_version: 1,
            option_id: "approve_replan".to_owned(),
            answered_by_user_id: owner_id.clone(),
            note: Some("Approved this exact repository change".to_owned()),
            answered_at: now.clone(),
        },
        None,
    )
    .await
    .expect("Human decision is answered exactly");
    db::TaskCoordinatorReplanRepo::transition_coordinator_replan(
        &db,
        db::TransitionCoordinatorReplan {
            id: replan_id.clone(),
            expected_version: 2,
            to_state: db::CoordinatorReplanState::AwaitingHuman,
            plan_request_id: Some(request_id.clone()),
            classification: Some(db::CoordinatorReplanClassification::ScopeOrAuthorityChange),
            classification_facts_json: Some(r#"{"repository_set_unchanged":false}"#.to_owned()),
            human_decision_id: Some(decision_id.clone()),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("replan records its exact Human decision");

    let resume_id = new_uuid_v4();
    let resume_brief = "{}".to_owned();
    let (resume_request, _) = TaskCoordinationRepo::create_task_coordination_request(
        &db,
        db::CreateTaskCoordinationRequestInput {
            id: resume_id.clone(),
            task_id: task_id.clone(),
            kind: db::TaskCoordinationRequestKind::HumanDecisionResume,
            review_report_artifact_id: None,
            review_report_digest: None,
            review_producer_execution_id: None,
            source_subject_kind: Some("human_decision".to_owned()),
            source_subject_id: Some(decision_id.clone()),
            source_subject_digest: Some(hex::encode(sha2::Sha256::digest(decision_id.as_bytes()))),
            resumes_request_id: Some(request_id.clone()),
            requested_by: owner_id.clone(),
            expected_task_version: task.version,
            authorization_version: coordination.authorization_version + 1,
            mode: TaskCoordinationMode::PrepareAndExecute,
            brief_json: resume_brief.clone(),
            objective_digest: hex::encode(sha2::Sha256::digest(resume_brief.as_bytes())),
            request_digest: hex::encode(sha2::Sha256::digest(b"resume-request")),
            idempotency_key: format!("human-decision-resume:{decision_id}"),
            created_at: now.clone(),
            event: event(
                "task.coordination_requested",
                "task_coordination_request",
                &resume_id,
                &task_id,
                &owner_id,
            ),
        },
    )
    .await
    .expect("exact Human resume request creates");
    assert_eq!(resume_request.id, resume_id);
    db::HumanDecisionRepo::bind_human_decision_resume_request(
        &db,
        &decision_id,
        2,
        &resume_id,
        &now,
    )
    .await
    .expect("decision binds the exact resume request");
    db::TaskCoordinatorReplanRepo::transition_coordinator_replan(
        &db,
        db::TransitionCoordinatorReplan {
            id: replan_id.clone(),
            expected_version: 3,
            to_state: db::CoordinatorReplanState::Approved,
            plan_request_id: None,
            classification: None,
            classification_facts_json: None,
            human_decision_id: None,
            updated_at: now.clone(),
        },
    )
    .await
    .expect("replan is approved");

    let historical_work_unit_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO work_unit (
            id, task_id, title, scope, status, role, assigned_actor_kind, assigned_actor_id,
            requires_integration, created_by_kind, created_by_id, version, created_at, updated_at
         ) VALUES (?, ?, 'Old work', 'Completed before replan', 'completed', 'implementer',
                   'human', ?, 0, 'human', ?, 1, ?, ?)",
    )
    .bind(&historical_work_unit_id)
    .bind(&task_id)
    .bind(&owner_id)
    .bind(&owner_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("historical WorkUnit inserts");
    TaskRepositorySetRepo::pin_work_unit(&db, &historical_work_unit_id, &task_id, current.revision)
        .await
        .expect("historical WorkUnit pins old revision");

    let work_unit_id = new_uuid_v4();
    let materialization = TaskCoordinationRepo::materialize_task_coordination_plan(
        &db,
        db::MaterializeTaskCoordinationPlanInput {
            receipt_id: new_uuid_v4(),
            task_id: task_id.clone(),
            request_id: request_id.clone(),
            plan_artifact_id: artifact_id.clone(),
            plan_digest: plan_digest.clone(),
            authorization_version: coordination.authorization_version + 1,
            expected_team_digest: team_digest,
            expected_plan_version: 1,
            task_version_before: task.version,
            expected_request_version: 1,
            expected_repository_set_revision: current.revision,
            expected_repository_set_digest: current.digest.clone(),
            repository_set_revision_change: Some(db::CreateTaskRepositorySetRevision {
                task_id: task_id.clone(),
                expected_current_revision: current.revision,
                expected_current_digest: current.digest.clone(),
                repositories: vec![
                    db::TaskRepositoryWrite {
                        repo_id: primary_repo_id.clone(),
                        role: db::TaskRepositoryRole::Primary,
                    },
                    db::TaskRepositoryWrite {
                        repo_id: added_repo_id.clone(),
                        role: db::TaskRepositoryRole::Affected,
                    },
                ],
                source_kind: "coordinator_replan".to_owned(),
                source_ref: Some(replan_id.clone()),
                decision_id: None,
                human_decision_request_id: Some(decision_id.clone()),
                force_new_revision: true,
                created_at: now.clone(),
            }),
            coordinator_replan_decision: Some(db::CoordinatorReplanDecisionProof {
                replan_id: replan_id.clone(),
                decision_id: decision_id.clone(),
                request_id: request_id.clone(),
                plan_artifact_id: artifact_id.clone(),
                plan_digest: plan_digest.clone(),
                authorization_version: coordination.authorization_version + 1,
                approved_by_user_id: owner_id.clone(),
                base_repository_set_revision: current.revision,
                base_repository_set_digest: current.digest.clone(),
                base_repo_id: Some(primary_repo_id.clone()),
            }),
            work_units: vec![db::MaterializedWorkUnit {
                id: work_unit_id.clone(),
                key: "new-work".to_owned(),
                title: "Work on the added repository".to_owned(),
                scope: "Implement the approved change".to_owned(),
                execution_instructions: None,
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Human(owner_id.clone())),
                parent_work_unit_id: None,
                depends_on_work_unit_ids: Vec::new(),
                requires_integration: false,
                artifact_inputs: Vec::new(),
                review_subject_work_unit_id: None,
            }],
            receipt_digest: hex::encode(sha2::Sha256::digest(b"replan-materialization")),
            created_by: owner_id.clone(),
            created_at: now.clone(),
            events: Vec::new(),
        },
    )
    .await
    .expect("approved resume materializes the new revision and WorkUnit");
    assert_eq!(
        materialization.work_unit_ids_json,
        format!("[\"{work_unit_id}\"]")
    );

    let old_pin: i64 = sqlx::query_scalar(
        "SELECT revision FROM work_unit_repository_set_revision WHERE work_unit_id = ?",
    )
    .bind(&historical_work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("old WorkUnit pin remains");
    let new_pin: i64 = sqlx::query_scalar(
        "SELECT revision FROM work_unit_repository_set_revision WHERE work_unit_id = ?",
    )
    .bind(&work_unit_id)
    .fetch_one(db.pool())
    .await
    .expect("new WorkUnit pin exists");
    assert_eq!(old_pin, current.revision);
    assert_eq!(new_pin, current.revision + 1);
    let historical = TaskRepositorySetRepo::get_revision(&db, &task_id, current.revision)
        .await
        .expect("historical revision loads")
        .expect("historical revision exists");
    assert_eq!(historical.repositories, current.repositories);
    let current_after = TaskRepositorySetRepo::get_current(&db, &task_id)
        .await
        .expect("new current revision loads")
        .expect("new current revision exists");
    assert_eq!(current_after.revision, current.revision + 1);
    assert_eq!(current_after.repositories.len(), 2);
    assert!(current_after
        .repositories
        .iter()
        .any(|repository| repository.repo_id == added_repo_id));
}
