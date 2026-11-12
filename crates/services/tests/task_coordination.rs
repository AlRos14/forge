use std::sync::Arc;

use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind, AgentRepo,
    AgentStatus, CoordinationMode, CreateAgentIdentity, CreateAgentProfile, CreateDomainEvent,
    CreateOrchestratorWake, CreateProjectMember, CreateRoleMembership, CreateTask, CreateTaskRole,
    CreateWorkUnit, DomainEventRepo, OrchestratorWakeRepo, ProjectMemberRepo, ProjectRepo,
    ProjectTaskRoleDefault, ProjectTaskRoleDefaultsRepo, RoleMembershipRepo, SqliteDb,
    TaskCoordinationMode, TaskRepo, TaskRoleRepo, UserRepo, WorkUnitProvenance, WorkUnitRepo,
    WorkUnitStatus,
};
use events::EventBus;
use services::{
    CoordinationBrief, InterventionInput, TaskCoordinationService, TaskQualityService, TaskService,
    TurnRequestInput, WorkspaceExecutionLockManager,
};

struct Fixture {
    db: Arc<SqliteDb>,
    service: TaskCoordinationService,
    task_id: String,
    user_id: String,
    role_id: String,
    task_version: i64,
}

async fn fixture(suffix: &str) -> Fixture {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let db = Arc::new(SqliteDb::new(pool));
    let now = now_rfc3339();
    let user_id = format!("quality-user-{suffix}");
    let agent_id = format!("quality-agent-{suffix}");
    let project_id = format!("quality-project-{suffix}");
    let task_id = format!("quality-task-{suffix}");
    let role_id = format!("quality-role-{suffix}");

    UserRepo::create_user(
        &*db,
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
    ProjectRepo::create(
        &*db,
        db::CreateProject {
            id: project_id.clone(),
            name: "Quality".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: Some(user_id.clone()),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project");
    AgentRepo::create_identity_with_profile(
        &*db,
        CreateAgentIdentity {
            id: agent_id.clone(),
            name: "Coordinator".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(user_id.clone()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: format!("{agent_id}-profile"),
            identity_id: agent_id.clone(),
            backend_kind: "native".to_owned(),
            executor_type: "codex".to_owned(),
            provider: Some("test".to_owned()),
            model: Some("test".to_owned()),
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "{}".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("agent");
    TaskRepo::create(
        &*db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: None,
            parent_task_id: None,
            subtask_order: None,
            title: "Quality routing".to_owned(),
            description: Some("objective".to_owned()),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task");
    TaskRoleRepo::create_with_configuration(
        &*db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: task_id.clone(),
            role: "planner".to_owned(),
            coordination_mode: Some(CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":"Planner","capabilities":["orchestration"],"rework":null}"#,
    )
    .await
    .expect("role");
    RoleMembershipRepo::add(
        &*db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role_id.clone(),
            actor_kind: ActorKind::Agent,
            actor_id: agent_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("membership");
    let event_bus = Arc::new(EventBus::new(32));
    let task_service = TaskService::new(Arc::clone(&db), Arc::clone(&event_bus));
    task_service
        .create_task_role_with_configuration(
            &task_id,
            "reviewer",
            CoordinationMode::Independent,
            "{}".to_owned(),
            r#"{"schema_version":1,"display_label":"Reviewer","capabilities":["formal_review"],"rework":null}"#.to_owned(),
        )
        .await
        .expect("reviewer TaskRole with formal_review capability creates through TaskService");
    task_service
        .add_task_role_member(
            &task_id,
            "reviewer",
            api_types::ActorRef::Agent(agent_id.clone()),
        )
        .await
        .expect("Agent becomes an active member of the formal reviewer role");
    let task = TaskRepo::get_by_id(&*db, &task_id, false)
        .await
        .expect("task")
        .expect("task row");
    let service = TaskCoordinationService::new(Arc::clone(&db), event_bus);
    let overview = service
        .authorize(
            &task_id,
            &user_id,
            services::AuthorizationInput {
                expected_task_version: task.version,
                expected_coordination_version: 1,
                mode: TaskCoordinationMode::Proposal,
                coordinator_task_role_id: Some(role_id.clone()),
                coordinator_actor: Some(db::ActorRef::Agent(agent_id.clone())),
                envelope: None,
                idempotency_key: "authorize".to_owned(),
            },
        )
        .await
        .expect("authorize");
    let task_version = overview.task.version;
    Fixture {
        db,
        service,
        task_id,
        user_id,
        role_id,
        task_version,
    }
}

async fn add_quality_role(fixture: &Fixture, role_id: &str, role_label: &str) {
    let now = now_rfc3339();
    TaskRoleRepo::create_with_configuration(
        &*fixture.db,
        CreateTaskRole {
            id: role_id.to_owned(),
            task_id: fixture.task_id.clone(),
            role: role_label.to_owned(),
            coordination_mode: Some(CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":null,"capabilities":[],"rework":null}"#,
    )
    .await
    .expect("quality role");
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role_id.to_owned(),
            actor_kind: ActorKind::Human,
            actor_id: fixture.user_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("quality role membership");
}

#[tokio::test]
async fn authorization_can_be_revoked_and_restored_repeatedly_with_exact_replay() {
    let fixture = fixture("reauthorization").await;
    for round in 0..3 {
        let current =
            db::TaskCoordinationRepo::get_task_coordination(&*fixture.db, &fixture.task_id)
                .await
                .expect("coordination")
                .expect("row");
        let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
            .await
            .expect("task")
            .expect("row");
        let revoked = fixture
            .service
            .revoke(
                &fixture.task_id,
                &fixture.user_id,
                task.version,
                current.authorization_version,
                Some("Review pause".to_owned()),
                &format!("revoke-{round}"),
            )
            .await
            .expect("repeated revocation");
        let input = services::AuthorizationInput {
            expected_task_version: revoked.task.version,
            expected_coordination_version: revoked.coordination.authorization_version,
            mode: TaskCoordinationMode::Proposal,
            coordinator_task_role_id: Some(fixture.role_id.clone()),
            coordinator_actor: Some(db::ActorRef::Agent(
                "quality-agent-reauthorization".to_owned(),
            )),
            envelope: None,
            idempotency_key: format!("restore-{round}"),
        };
        let restored = fixture
            .service
            .authorize(&fixture.task_id, &fixture.user_id, input.clone())
            .await
            .expect("restore authorization");
        let replay = fixture
            .service
            .authorize(&fixture.task_id, &fixture.user_id, input)
            .await
            .expect("replay authorization");
        assert!(restored.coordination.revoked_at.is_none());
        assert_eq!(
            restored.coordination.authorization_version,
            current.authorization_version + 2
        );
        assert_eq!(
            replay.coordination.authorization_version,
            restored.coordination.authorization_version
        );
    }
    let events: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE scope_id = ?
         AND event_type IN ('task.coordination_authorized', 'task.coordination_revoked')",
    )
    .bind(&fixture.task_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("events");
    assert_eq!(events, 7, "replays must not create authorization events");
}

#[tokio::test]
async fn quality_handoff_preserves_a_consumer_advance_and_replay_budget() {
    let fixture = fixture("handoff-race").await;
    add_quality_role(&fixture, "quality-race-target", "custom-worker").await;
    // Advance the request exactly at the notification persistence boundary,
    // after the creator has read its original version. This deterministically
    // models a durable consumer winning the race without timing assumptions.
    sqlx::query(
        "CREATE TRIGGER review_consumer_wins_handoff AFTER INSERT ON handoff
         BEGIN
           UPDATE task_quality_request SET state = 'requires_human', version = version + 1,
             requires_human_reason_code = 'coordination.target_missing',
             requires_human_reason = 'Consumer requires an exact correction'
           WHERE task_id = NEW.task_id AND state = 'requested';
         END",
    )
    .execute(fixture.db.pool())
    .await
    .expect("race barrier");
    let input = services::QualityRequestInput {
        source_artifact_id: None,
        category: db::TaskQualityCategory::DeliveryCorrection,
        target_kind: db::TaskQualityTargetKind::Implementation,
        target_task_role_id: Some("quality-race-target".to_owned()),
        target_actor: None,
        summary: "Correct the exact delivery".to_owned(),
        findings: vec!["Independent review finding".to_owned()],
        resolution_condition: "Exact correction is independently reviewed".to_owned(),
        idempotency_key: "quality-handoff-race".to_owned(),
    };
    let created = fixture
        .service
        .create_quality_request(&fixture.task_id, &fixture.user_id, input.clone())
        .await
        .expect("creation succeeds after consumer advance");
    let replay = fixture
        .service
        .create_quality_request(&fixture.task_id, &fixture.user_id, input)
        .await
        .expect("replay");
    assert_eq!(created.state, db::TaskQualityState::RequiresHuman);
    assert_eq!(
        created.requires_human_reason_code.as_deref(),
        Some("coordination.target_missing")
    );
    assert!(created.handoff_id.is_some());
    assert_eq!(created.handoff_id, replay.handoff_id);
    assert_eq!(created.id, replay.id);
    let coordination =
        db::TaskCoordinationRepo::get_task_coordination(&*fixture.db, &fixture.task_id)
            .await
            .expect("coordination")
            .expect("row");
    assert_eq!(coordination.quality_request_count, 1);
    let handoffs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM handoff WHERE task_id = ?")
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("handoffs");
    assert_eq!(handoffs, 1);
}

#[tokio::test]
async fn request_turn_replay_compares_the_received_brief() {
    let fixture = fixture("replay").await;
    let first = fixture
        .service
        .request_turn(
            &fixture.task_id,
            &fixture.user_id,
            TurnRequestInput {
                expected_task_version: fixture.task_version,
                expected_coordination_version: 2,
                brief: CoordinationBrief {
                    objective: "Deliver the first plan".to_owned(),
                    ..CoordinationBrief::default()
                },
                idempotency_key: "same-key".to_owned(),
            },
        )
        .await
        .expect("first request");
    assert!(first.1);
    let replay = fixture
        .service
        .request_turn(
            &fixture.task_id,
            &fixture.user_id,
            TurnRequestInput {
                expected_task_version: fixture.task_version,
                expected_coordination_version: 2,
                brief: CoordinationBrief {
                    objective: "Deliver the first plan".to_owned(),
                    ..CoordinationBrief::default()
                },
                idempotency_key: "same-key".to_owned(),
            },
        )
        .await
        .expect("identical replay");
    assert!(!replay.1);
    assert_eq!(replay.0.id, first.0.id);

    let conflict = fixture
        .service
        .request_turn(
            &fixture.task_id,
            &fixture.user_id,
            TurnRequestInput {
                expected_task_version: fixture.task_version,
                expected_coordination_version: 2,
                brief: CoordinationBrief {
                    objective: "A different objective".to_owned(),
                    ..CoordinationBrief::default()
                },
                idempotency_key: "same-key".to_owned(),
            },
        )
        .await
        .expect_err("same key with a different brief must conflict");
    assert!(conflict.to_string().contains("idempotency_conflict"));
}

#[tokio::test]
async fn recorded_instruction_replay_reuses_its_exact_message() {
    let fixture = fixture("intervention").await;
    let input = || InterventionInput {
        kind: db::TaskInterventionKind::Instruction,
        instruction: Some("Keep the final API stable".to_owned()),
        plan_artifact_id: None,
        approve: None,
        rationale: None,
        reason: None,
        expected_task_version: Some(fixture.task_version),
        expected_lifecycle_version: None,
        expected_coordination_version: None,
        brief: None,
        idempotency_key: "instruction-key".to_owned(),
    };
    let applied = fixture
        .service
        .create_intervention(&fixture.task_id, &fixture.user_id, input())
        .await
        .expect("instruction");
    assert_eq!(applied.state, db::TaskInterventionState::Applied);
    let messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message WHERE task_id = ? AND sender_actor_id = ?",
    )
    .bind(&fixture.task_id)
    .bind(&fixture.user_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("messages");
    assert_eq!(messages, 1);
    // Simulate a crash after the durable record and effect committed but
    // before the applied-state event/row advanced. Replay must resume the
    // deterministic message identity, not create a second instruction.
    sqlx::query(
        "DELETE FROM domain_event WHERE event_type = 'task.intervention_state_changed' AND entity_id = ?",
    )
    .bind(&applied.id)
    .execute(fixture.db.pool())
    .await
    .expect("remove applied receipt");
    sqlx::query(
        "UPDATE task_intervention SET state = 'recorded', version = version + 1 WHERE id = ?",
    )
    .bind(&applied.id)
    .execute(fixture.db.pool())
    .await
    .expect("recorded state");
    let replayed = fixture
        .service
        .create_intervention(&fixture.task_id, &fixture.user_id, input())
        .await
        .expect("replay");
    assert_eq!(replayed.id, applied.id);
    assert_eq!(replayed.state, db::TaskInterventionState::Applied);
    let messages: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM message WHERE task_id = ? AND sender_actor_id = ?",
    )
    .bind(&fixture.task_id)
    .bind(&fixture.user_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("messages");
    assert_eq!(messages, 1);
}

#[tokio::test]
async fn team_mutation_marks_the_authorization_stale() {
    let fixture = fixture("stale").await;
    let coordination =
        db::TaskCoordinationRepo::get_task_coordination(&*fixture.db, &fixture.task_id)
            .await
            .expect("coordination")
            .expect("coordination row");
    assert!(!coordination.team_stale);
    // Updating the role itself is the representative team mutation.
    sqlx::query("UPDATE task_role SET version = version + 1 WHERE id = ?")
        .bind(&fixture.role_id)
        .execute(fixture.db.pool())
        .await
        .expect("role update");
    let coordination =
        db::TaskCoordinationRepo::get_task_coordination(&*fixture.db, &fixture.task_id)
            .await
            .expect("coordination")
            .expect("coordination row");
    assert!(coordination.team_stale);
    let error = fixture
        .service
        .request_turn(
            &fixture.task_id,
            &fixture.user_id,
            TurnRequestInput {
                expected_task_version: fixture.task_version,
                expected_coordination_version: 2,
                brief: CoordinationBrief {
                    objective: "Reject the stale team".to_owned(),
                    ..CoordinationBrief::default()
                },
                idempotency_key: "stale-key".to_owned(),
            },
        )
        .await
        .expect_err("stale team is rejected");
    assert!(error.to_string().contains("authorization_stale"));
}

#[tokio::test]
async fn coordination_wake_and_request_link_in_one_transaction() {
    let fixture = fixture("wake-link").await;
    let (request, _) = fixture
        .service
        .request_turn(
            &fixture.task_id,
            &fixture.user_id,
            TurnRequestInput {
                expected_task_version: fixture.task_version,
                expected_coordination_version: 2,
                brief: CoordinationBrief {
                    objective: "Link the wake atomically".to_owned(),
                    ..CoordinationBrief::default()
                },
                idempotency_key: "wake-link".to_owned(),
            },
        )
        .await
        .expect("turn request");
    let event = DomainEventRepo::get_event_by_dedupe(
        &*fixture.db,
        &format!("task.coordination_requested:{}", request.id),
    )
    .await
    .expect("event")
    .expect("coordination event");
    let role = TaskRoleRepo::get_by_id(&*fixture.db, &fixture.role_id)
        .await
        .expect("role")
        .expect("role row");
    let now = now_rfc3339();
    let wake = |id: String| CreateOrchestratorWake {
        id,
        event_id: event.id.clone(),
        event_sequence: event.sequence,
        task_id: fixture.task_id.clone(),
        task_role_id: fixture.role_id.clone(),
        coordination_mode: role.coordination_mode,
        actor_kind: ActorKind::Agent,
        actor_id: "quality-agent-wake-link".to_owned(),
        work_unit_id: None,
        correlation_id: event.correlation_id.clone(),
        causation_id: Some(event.id.clone()),
        causation_depth: event.causation_depth,
        policy_ref: "forge.orchestrator.wake-policy".to_owned(),
        policy_version: 1,
        policy_digest: "digest".to_owned(),
        task_role_version: role.version,
        task_role_policy_json: role.policy_json.clone(),
        coordination_request_id: Some(request.id.clone()),
        available_at: now.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    assert!(
        OrchestratorWakeRepo::admit_orchestrator_wake(&*fixture.db, wake(new_uuid_v4()))
            .await
            .expect("admit")
    );
    let linked = db::TaskCoordinationRepo::get_task_coordination_request(&*fixture.db, &request.id)
        .await
        .expect("request")
        .expect("request row");
    assert!(linked.wake_id.is_some());
    let wake_id = linked.wake_id.clone();
    assert!(
        !OrchestratorWakeRepo::admit_orchestrator_wake(&*fixture.db, wake(new_uuid_v4()))
            .await
            .expect("replay")
    );
    let replayed =
        db::TaskCoordinationRepo::get_task_coordination_request(&*fixture.db, &request.id)
            .await
            .expect("request")
            .expect("request row");
    assert_eq!(replayed.wake_id, wake_id);
}

#[tokio::test]
async fn interrupted_cancel_replay_records_the_pending_dispatch_receipt() {
    use sha2::{Digest, Sha256};

    let fixture = fixture("cancel-replay").await;
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
        .await
        .expect("lifecycle")
        .expect("lifecycle row");
    let input = || InterventionInput {
        kind: db::TaskInterventionKind::Cancel,
        instruction: None,
        plan_artifact_id: None,
        approve: None,
        rationale: None,
        reason: Some("cancel for replay coverage".to_owned()),
        expected_task_version: Some(fixture.task_version),
        expected_lifecycle_version: Some(lifecycle.version),
        expected_coordination_version: None,
        brief: None,
        idempotency_key: "cancel-replay".to_owned(),
    };
    let intervention_id = new_uuid_v4();
    let payload_json = serde_json::json!({
        "kind": "cancel",
        "instruction": null,
        "plan_artifact_id": null,
        "decision": null,
        "rationale": null,
        "reason": "cancel for replay coverage",
        "expected_lifecycle_version": lifecycle.version,
        "brief": null,
    })
    .to_string();
    let request_digest = hex::encode(Sha256::digest(payload_json.as_bytes()));
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO task_intervention (
             id, task_id, kind, state, source_kind, source_id, payload_json,
             request_digest, idempotency_key, expected_task_version,
             version, created_at, updated_at
         ) VALUES (?, ?, 'cancel', 'recorded', 'human', ?, ?, ?, 'cancel-replay', ?, 1, ?, ?)",
    )
    .bind(&intervention_id)
    .bind(&fixture.task_id)
    .bind(&fixture.user_id)
    .bind(&payload_json)
    .bind(&request_digest)
    .bind(fixture.task_version)
    .bind(&now)
    .bind(&now)
    .execute(fixture.db.pool())
    .await
    .expect("recorded intervention");
    // The crash window leaves TaskLifecycle cancelled and the dispatch receipt
    // absent. The durable intervention record is the only in-flight fact.
    let lifecycle_service = services::task_lifecycle::TaskLifecycleService::new(
        Arc::clone(&fixture.db),
        Arc::new(EventBus::new(32)),
    );
    lifecycle_service
        .transition_at_lifecycle_version(
            services::task_lifecycle::TransitionLifecycleInput {
                task_id: fixture.task_id.clone(),
                expected_task_version: fixture.task_version,
                to_state: db::TaskLifecycleState::Cancelled,
                cause: services::task_lifecycle::LifecycleCause::Actor(api_types::Actor::User {
                    user_id: Some(fixture.user_id.clone()),
                    source: api_types::UserActionSource::Api,
                }),
                reason_kind: Some("intervention".to_owned()),
                reason_ref: Some(intervention_id.clone()),
                idempotency_key: format!("intervention-cancel:{intervention_id}"),
            },
            lifecycle.version,
        )
        .await
        .expect("lifecycle cancel");
    let dispatch_key = format!("intervention:{intervention_id}");
    assert!(db::TaskLifecycleRepo::get_task_dispatch_receipt(
        &*fixture.db,
        &fixture.task_id,
        &dispatch_key
    )
    .await
    .expect("dispatch receipt")
    .is_none());

    let replayed = fixture
        .service
        .create_intervention(&fixture.task_id, &fixture.user_id, input())
        .await
        .expect("cancel replay");
    assert_eq!(replayed.id, intervention_id);
    assert_eq!(replayed.state, db::TaskInterventionState::Applied);
    assert!(db::TaskLifecycleRepo::get_task_dispatch_receipt(
        &*fixture.db,
        &fixture.task_id,
        &dispatch_key
    )
    .await
    .expect("dispatch receipt")
    .is_some());
    let cancelled = db::TaskLifecycleRepo::get_task_lifecycle(&*fixture.db, &fixture.task_id)
        .await
        .expect("lifecycle")
        .expect("lifecycle row");
    assert_eq!(cancelled.state, db::TaskLifecycleState::Cancelled);
}

#[tokio::test]
async fn plan_materialized_delivery_completes_after_exact_execution_result() {
    use sha2::{Digest, Sha256};

    let fixture = fixture("delivery-completion").await;
    let specialist_role_id = "delivery-specialist-role".to_owned();
    let now = now_rfc3339();
    TaskRoleRepo::create_with_configuration(
        &*fixture.db,
        CreateTaskRole {
            id: specialist_role_id.clone(),
            task_id: fixture.task_id.clone(),
            role: "specialist".to_owned(),
            coordination_mode: Some(CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":"Specialist","capabilities":[],"rework":null}"#,
    )
    .await
    .expect("specialist role");
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: specialist_role_id,
            actor_kind: ActorKind::Human,
            actor_id: fixture.user_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("specialist membership");
    sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .expect("refresh team staleness");
    let plan_artifact_id = "delivery-plan".to_owned();
    let producer_execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, summary, created_at, updated_at,
             actor_kind, actor_id
         ) VALUES (?, ?, NULL, 'implementer', 'completed', '{}', ?, ?, 'human', ?)",
    )
    .bind(&producer_execution_id)
    .bind(&fixture.task_id)
    .bind(&now)
    .bind(&now)
    .bind(&fixture.user_id)
    .execute(fixture.db.pool())
    .await
    .expect("plan producer execution");
    let mut artifact_tx = fixture
        .db
        .pool()
        .begin()
        .await
        .expect("artifact transaction");
    sqlx::query(
        "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
         VALUES (?, ?, ?)",
    )
    .bind(&plan_artifact_id)
    .bind(&producer_execution_id)
    .bind(&fixture.task_id)
    .execute(&mut *artifact_tx)
    .await
    .expect("plan producer");
    sqlx::query(
        "INSERT INTO artifact (
             id, task_id, kind, storage_kind, content, content_ref,
             metadata_json, digest, created_at
         ) VALUES (?, ?, 'work_unit_plan', 'inline', '{}', NULL, '{}', ?, ?)",
    )
    .bind(&plan_artifact_id)
    .bind(&fixture.task_id)
    .bind(hex::encode(Sha256::digest(b"{}")))
    .bind(&now)
    .execute(&mut *artifact_tx)
    .await
    .expect("plan artifact");
    artifact_tx.commit().await.expect("artifact commit");
    sqlx::query(
        "INSERT INTO execution_artifact_output (
             execution_id, artifact_id, task_id, kind, digest, created_at
         ) VALUES (?, ?, ?, 'work_unit_plan', ?, ?)",
    )
    .bind(&producer_execution_id)
    .bind(&plan_artifact_id)
    .bind(&fixture.task_id)
    .bind(hex::encode(Sha256::digest(b"{}")))
    .bind(&now)
    .execute(fixture.db.pool())
    .await
    .expect("plan output");
    let unit_id = "delivery-unit".to_owned();
    WorkUnitRepo::create(
        &*fixture.db,
        CreateWorkUnit {
            id: unit_id.clone(),
            task_id: fixture.task_id.clone(),
            parent_work_unit_id: None,
            title: "Deliver".to_owned(),
            scope: "Produce the exact result".to_owned(),
            execution_instructions: None,
            role: "specialist".to_owned(),
            assigned_actor: Some(db::ActorRef::Human(fixture.user_id.clone())),
            requires_integration: false,
            provenance: Some(WorkUnitProvenance::Artifact(plan_artifact_id)),
            artifact_input_ids: Vec::new(),
            review_approval_gate_id: None,
            review_approval_line_id: None,
            created_by: db::ActorRef::Human(fixture.user_id.clone()),
            created_at: now.clone(),
        },
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: unit_id.clone(),
            actor_type: "human".to_owned(),
            actor_id: Some(fixture.user_id.clone()),
            scope_type: "task".to_owned(),
            scope_id: fixture.task_id.clone(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("delivery-work-unit:{unit_id}")),
            payload_json: serde_json::json!({
                "task_id": fixture.task_id,
                "work_unit_id": unit_id,
                "version": 1,
                "status": "open",
            })
            .to_string(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("work unit");
    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, work_unit_id, work_unit_version
         ) VALUES (?, ?, NULL, 'specialist', 'running', ?, ?, 'human', ?, ?, 1)",
    )
    .bind(&execution_id)
    .bind(&fixture.task_id)
    .bind(&now)
    .bind(&now)
    .bind(&fixture.user_id)
    .bind(&unit_id)
    .execute(fixture.db.pool())
    .await
    .expect("running execution");
    sqlx::query(
        "UPDATE execution SET status = 'completed', after_sha = 'exact-result-sha',
             updated_at = ? WHERE id = ?",
    )
    .bind(now_rfc3339())
    .bind(&execution_id)
    .execute(fixture.db.pool())
    .await
    .expect("completed execution");
    let task_service = TaskService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)))
        .with_workspace_root(std::env::temp_dir())
        .with_workspace_exec_locks(Arc::new(WorkspaceExecutionLockManager::new()))
        .with_repo_cache_locks(Arc::new(workspace::RepoCacheLockManager::new()));
    let completed = task_service
        .complete_coordination_delivery_for_execution(&execution_id)
        .await
        .expect("delivery completion")
        .expect("completed WorkUnit");
    assert_eq!(completed.id, unit_id);
    assert_eq!(completed.status, WorkUnitStatus::Completed);
}

#[tokio::test]
async fn delivery_correction_without_exact_rework_escalates_instead_of_faking_work() {
    let fixture = fixture("quality-no-rework").await;
    let quality = TaskQualityService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));
    let request = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            services::QualityRequestInput {
                source_artifact_id: None,
                category: db::TaskQualityCategory::DeliveryCorrection,
                target_kind: db::TaskQualityTargetKind::Implementation,
                target_task_role_id: Some(fixture.role_id.clone()),
                target_actor: None,
                summary: "Correct the delivery".to_owned(),
                findings: Vec::new(),
                resolution_condition: "The exact delivery is corrected and reviewed".to_owned(),
                idempotency_key: "no-rework".to_owned(),
            },
        )
        .await
        .expect("quality request");
    quality.process_pending(10).await.expect("quality consumer");
    let request = db::TaskCoordinationRepo::get_task_quality_request(&*fixture.db, &request.id)
        .await
        .expect("request")
        .expect("request row");
    assert_eq!(request.state, db::TaskQualityState::RequiresHuman);
    assert!(request.work_unit_id.is_none());
    assert_eq!(
        request.requires_human_reason_code.as_deref(),
        Some("coordination.rework_unavailable")
    );
    assert!(request
        .requires_human_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("exact ReviewReport")));
    let units: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM work_unit WHERE task_id = ?")
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("units");
    assert_eq!(units, 0);
}

#[tokio::test]
async fn rework_correction_completion_freezes_result_for_its_review() {
    use sha2::{Digest, Sha256};

    let fixture = fixture("rework-completion").await;
    let now = now_rfc3339();
    let reviewer_user_id = "rework-reviewer".to_owned();
    UserRepo::create_user(
        &*fixture.db,
        &db::User {
            id: reviewer_user_id.clone(),
            email: "rework-reviewer@example.invalid".to_owned(),
            password_hash: "unused".to_owned(),
            display_name: Some("Reviewer".to_owned()),
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("reviewer user");
    let specialist_role_id = "rework-specialist-role".to_owned();
    TaskRoleRepo::create_with_configuration(
        &*fixture.db,
        CreateTaskRole {
            id: specialist_role_id,
            task_id: fixture.task_id.clone(),
            role: "specialist".to_owned(),
            coordination_mode: Some(CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":"Specialist","capabilities":[],"rework":null}"#,
    )
    .await
    .expect("specialist role");
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: "rework-specialist-role".to_owned(),
            actor_kind: ActorKind::Human,
            actor_id: fixture.user_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("specialist membership");
    let task_project_id: String = sqlx::query_scalar("SELECT project_id FROM task WHERE id=?")
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("task project ID");
    ProjectMemberRepo::add_member(
        &*fixture.db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: task_project_id,
            user_id: reviewer_user_id.clone(),
            role: "member".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("human reviewer becomes a current Project member");
    TaskService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)))
        .add_task_role_member(
            &fixture.task_id,
            "reviewer",
            api_types::ActorRef::Human(reviewer_user_id.clone()),
        )
        .await
        .expect("human reviewer joins the existing formal_review TaskRole");
    sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .expect("refresh team staleness");
    let report_id = "rework-report".to_owned();
    let producer_execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, summary, created_at, updated_at,
             actor_kind, actor_id
         ) VALUES (?, ?, NULL, 'implementer', 'completed', '{}', ?, ?, 'human', ?)",
    )
    .bind(&producer_execution_id)
    .bind(&fixture.task_id)
    .bind(&now)
    .bind(&now)
    .bind(&fixture.user_id)
    .execute(fixture.db.pool())
    .await
    .expect("report producer execution");
    let mut tx = fixture
        .db
        .pool()
        .begin()
        .await
        .expect("artifact transaction");
    sqlx::query(
        "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
         VALUES (?, ?, ?)",
    )
    .bind(&report_id)
    .bind(&producer_execution_id)
    .bind(&fixture.task_id)
    .execute(&mut *tx)
    .await
    .expect("report producer");
    sqlx::query(
        "INSERT INTO artifact (
             id, task_id, kind, storage_kind, content, content_ref,
             metadata_json, digest, created_at
         ) VALUES (?, ?, 'summary', 'inline', '{}', NULL, '{}', ?, ?)",
    )
    .bind(&report_id)
    .bind(&fixture.task_id)
    .bind(hex::encode(Sha256::digest(b"{}")))
    .bind(&now)
    .execute(&mut *tx)
    .await
    .expect("report artifact");
    tx.commit().await.expect("artifact commit");

    let correction_id = "rework-correction".to_owned();
    let review_id = "rework-review".to_owned();
    for (id, title, role) in [
        (&correction_id, "Correct", "specialist"),
        (&review_id, "Review correction", "reviewer"),
    ] {
        let assigned_actor = if role == "reviewer" {
            reviewer_user_id.clone()
        } else {
            fixture.user_id.clone()
        };
        WorkUnitRepo::create(
            &*fixture.db,
            CreateWorkUnit {
                id: id.clone(),
                task_id: fixture.task_id.clone(),
                parent_work_unit_id: None,
                title: title.to_owned(),
                scope: title.to_owned(),
                execution_instructions: None,
                role: role.to_owned(),
                assigned_actor: Some(db::ActorRef::Human(assigned_actor)),
                requires_integration: false,
                provenance: Some(WorkUnitProvenance::Artifact(report_id.clone())),
                artifact_input_ids: Vec::new(),
                review_approval_gate_id: None,
                review_approval_line_id: None,
                created_by: db::ActorRef::Human(fixture.user_id.clone()),
                created_at: now.clone(),
            },
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "work_unit.created".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: id.clone(),
                actor_type: "human".to_owned(),
                actor_id: Some(fixture.user_id.clone()),
                scope_type: "task".to_owned(),
                scope_id: fixture.task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("rework-work-unit:{id}")),
                payload_json: serde_json::json!({
                    "task_id": fixture.task_id,
                    "work_unit_id": id,
                    "version": 1,
                    "status": "open",
                })
                .to_string(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("work unit");
    }
    sqlx::query(
        "INSERT INTO work_unit_review_subject (
             review_work_unit_id, task_id, reviewed_work_unit_id,
             reviewed_work_unit_version, result_execution_id, result_artifact_id,
             result_commit_sha, result_workspace_id, workspace_snapshot_digest, created_at
         ) VALUES (?, ?, ?, NULL, NULL, NULL, NULL, NULL, NULL, ?)",
    )
    .bind(&review_id)
    .bind(&fixture.task_id)
    .bind(&correction_id)
    .bind(&now)
    .execute(fixture.db.pool())
    .await
    .expect("review subject");
    let quality_route_id = "source:legacy-rework-completion";
    sqlx::query(
        "INSERT INTO task_quality_route (
             task_id, route_id, route_kind, route_ref, request_count, budget_used,
             max_requests, created_at, updated_at
         ) VALUES (?, ?, 'source', ?, 1, 1, 4, ?, ?)",
    )
    .bind(&fixture.task_id)
    .bind(quality_route_id)
    .bind(&fixture.user_id)
    .bind(&now)
    .bind(&now)
    .execute(fixture.db.pool())
    .await
    .expect("quality route fixture");
    sqlx::query(
        "INSERT INTO task_quality_request (
             id, task_id, source_kind, source_id, category, target_kind,
             target_task_role_id, target_actor_kind, target_actor_id, summary,
             findings_json, resolution_condition, state, handoff_id, work_unit_id,
             budget_round, source_digest, request_digest, idempotency_key,
             created_by_kind, created_by_id, version, created_at, updated_at,
             route_id, route_round, route_limit
         ) VALUES (?, ?, 'human', ?, 'delivery_correction', 'implementation',
                   ?, 'human', ?, 'Correct the delivery', '[]',
                   'The correction is reviewed', 'work_created', NULL, ?,
                   1, ?, ?, 'rework-completion-quality', 'human', ?, 1, ?, ?, ?, 1, 4)",
    )
    .bind(new_uuid_v4())
    .bind(&fixture.task_id)
    .bind(&fixture.user_id)
    .bind("rework-specialist-role")
    .bind(&fixture.user_id)
    .bind(&correction_id)
    .bind(hex::encode(Sha256::digest(b"source")))
    .bind(hex::encode(Sha256::digest(b"request")))
    .bind(&fixture.user_id)
    .bind(&now)
    .bind(&now)
    .bind(quality_route_id)
    .execute(fixture.db.pool())
    .await
    .expect("quality request link");
    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, work_unit_id, work_unit_version
         ) VALUES (?, ?, NULL, 'specialist', 'running', ?, ?, 'human', ?, ?, 1)",
    )
    .bind(&execution_id)
    .bind(&fixture.task_id)
    .bind(&now)
    .bind(&now)
    .bind(&fixture.user_id)
    .bind(&correction_id)
    .execute(fixture.db.pool())
    .await
    .expect("correction execution");
    sqlx::query(
        "UPDATE execution SET status = 'completed', after_sha = 'correction-sha',
             updated_at = ? WHERE id = ?",
    )
    .bind(now_rfc3339())
    .bind(&execution_id)
    .execute(fixture.db.pool())
    .await
    .expect("completed correction execution");
    let task_service = TaskService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)))
        .with_workspace_root(std::env::temp_dir())
        .with_workspace_exec_locks(Arc::new(WorkspaceExecutionLockManager::new()))
        .with_repo_cache_locks(Arc::new(workspace::RepoCacheLockManager::new()));
    sqlx::query("UPDATE task_coordination SET mode = 'manual' WHERE task_id = ?")
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .expect("manual mode");
    assert!(task_service
        .complete_coordination_delivery_for_execution(&execution_id)
        .await
        .expect("manual no-op")
        .is_none());
    sqlx::query("UPDATE task_coordination SET mode = 'proposal', revoked_at = ? WHERE task_id = ?")
        .bind(&now)
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .expect("revoked mode");
    assert!(task_service
        .complete_coordination_delivery_for_execution(&execution_id)
        .await
        .expect("revoked no-op")
        .is_none());
    sqlx::query("UPDATE task_coordination SET revoked_at = NULL, team_stale = 0 WHERE task_id = ?")
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .expect("restore authorization");
    let completed = task_service
        .complete_coordination_delivery_for_execution(&execution_id)
        .await
        .expect("correction completion")
        .expect("completed correction");
    assert_eq!(completed.id, correction_id);
    assert_eq!(completed.status, WorkUnitStatus::Completed);
    let line: (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT id, latest_verdict, current_commit_sha FROM work_unit_review_line
         WHERE task_id = ? AND root_work_unit_id = ?",
    )
    .bind(&fixture.task_id)
    .bind(&correction_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("correction review line");
    assert!(line.1.is_none());
    assert_eq!(line.2.as_deref(), Some("correction-sha"));
}

#[tokio::test]
async fn quality_route_budget_is_atomic_and_does_not_reset_with_category_or_role() {
    let fixture = fixture("quality-route-budget").await;
    sqlx::query("UPDATE task_autonomy_limits SET max_route_requests=4 WHERE task_id=?")
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .unwrap();

    let role_a = "quality-route-role-a";
    add_quality_role(&fixture, role_a, "specialist-alpha").await;
    let quality = TaskQualityService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));

    let mut first_route_id = None;
    for round in 1..=4 {
        let request = fixture
            .service
            .create_quality_request(
                &fixture.task_id,
                &fixture.user_id,
                services::QualityRequestInput {
                    source_artifact_id: None,
                    category: db::TaskQualityCategory::Specialist,
                    target_kind: db::TaskQualityTargetKind::Specialist,
                    target_task_role_id: Some(role_a.to_owned()),
                    target_actor: None,
                    summary: "same durable quality issue".to_owned(),
                    findings: vec![format!("finding {round}")],
                    resolution_condition: "the issue is resolved".to_owned(),
                    idempotency_key: format!("quality-route-{round}"),
                },
            )
            .await
            .expect("quality request within route budget");
        assert_eq!(request.route_round, round);
        assert_eq!(request.route_limit, 4);
        assert!(request.requires_human_reason_code.is_none());
        if let Some(route_id) = first_route_id.as_ref() {
            assert_eq!(&request.route_id, route_id);
        } else {
            first_route_id = Some(request.route_id.clone());
        }
        assert_eq!(request.state, db::TaskQualityState::Notified);
    }

    // The fifth request changes both category and target TaskRole. The exact
    // same source chain still reaches its persisted route limit.
    let exhausted = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            services::QualityRequestInput {
                source_artifact_id: None,
                category: db::TaskQualityCategory::PlanRevision,
                target_kind: db::TaskQualityTargetKind::Orchestrator,
                target_task_role_id: Some(fixture.role_id.clone()),
                target_actor: None,
                summary: "same durable quality issue".to_owned(),
                findings: Vec::new(),
                resolution_condition: "the plan revision is inspected".to_owned(),
                idempotency_key: "quality-route-5".to_owned(),
            },
        )
        .await
        .expect("route exhaustion is recorded for Human handling");
    assert_eq!(exhausted.route_id, first_route_id.expect("route ID"));
    assert_eq!(exhausted.route_round, 5);
    assert_eq!(exhausted.route_limit, 4);
    assert_eq!(exhausted.state, db::TaskQualityState::RequiresHuman);
    assert_eq!(
        exhausted.requires_human_reason_code.as_deref(),
        Some("coordination.quality_route_budget_exhausted")
    );
    assert!(exhausted
        .requires_human_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("4 of 4")));

    let replay = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            services::QualityRequestInput {
                source_artifact_id: None,
                category: db::TaskQualityCategory::PlanRevision,
                target_kind: db::TaskQualityTargetKind::Orchestrator,
                target_task_role_id: Some(fixture.role_id.clone()),
                target_actor: None,
                summary: "same durable quality issue".to_owned(),
                findings: Vec::new(),
                resolution_condition: "the plan revision is inspected".to_owned(),
                idempotency_key: "quality-route-5".to_owned(),
            },
        )
        .await
        .expect("idempotent replay");
    assert_eq!(replay.id, exhausted.id);
    let counts: (i64, i64) = sqlx::query_as(
        "SELECT request_count, budget_used FROM task_quality_route
         WHERE task_id = ? AND route_id = ?",
    )
    .bind(&fixture.task_id)
    .bind(&exhausted.route_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("route counters");
    assert_eq!(counts, (5, 4));
    assert_eq!(quality.process_pending(10).await.expect("recovery scan"), 4);
    let autonomy = services::task_autonomy::TaskAutonomyService::new(
        fixture.db.clone(),
        Arc::new(EventBus::new(32)),
    );
    let mut limits = autonomy
        .get(&fixture.task_id, &fixture.user_id)
        .await
        .unwrap();
    let version = limits.version;
    limits.max_route_requests = 12;
    let grant = api_types::UpdateTaskAutonomyLimitsRequest {
        expected_version: version,
        limits,
        reason: "Human authorized more quality rounds".into(),
        idempotency_key: "extend-route".into(),
    };
    autonomy
        .update(&fixture.task_id, &fixture.user_id, grant.clone())
        .await
        .unwrap();
    autonomy
        .update(&fixture.task_id, &fixture.user_id, grant)
        .await
        .unwrap();
    let state: String = sqlx::query_scalar("SELECT state FROM task_quality_request WHERE id=?")
        .bind(&exhausted.id)
        .fetch_one(fixture.db.pool())
        .await
        .unwrap();
    assert_eq!(state, "requested");
    let counts:(i64,i64,i64)=sqlx::query_as("SELECT request_count,budget_used,max_requests FROM task_quality_route WHERE task_id=? AND route_id=?")
        .bind(&fixture.task_id).bind(&exhausted.route_id).fetch_one(fixture.db.pool()).await.unwrap();
    assert_eq!(
        counts,
        (5, 5, 12),
        "extension and exact replay preserve consumed rounds and request identity"
    );
}

#[tokio::test]
async fn concurrent_quality_route_requests_consume_one_round_each_and_replay_once() {
    let fixture = fixture("quality-route-concurrency").await;
    sqlx::query("UPDATE task_autonomy_limits SET max_route_requests=4 WHERE task_id=?")
        .bind(&fixture.task_id)
        .execute(fixture.db.pool())
        .await
        .unwrap();

    let role_id = "quality-route-concurrent-role";
    add_quality_role(&fixture, role_id, "custom-specialist").await;
    for round in 1..=3 {
        fixture
            .service
            .create_quality_request(
                &fixture.task_id,
                &fixture.user_id,
                services::QualityRequestInput {
                    source_artifact_id: None,
                    category: db::TaskQualityCategory::Specialist,
                    target_kind: db::TaskQualityTargetKind::Specialist,
                    target_task_role_id: Some(role_id.to_owned()),
                    target_actor: None,
                    summary: "concurrent route chain".to_owned(),
                    findings: Vec::new(),
                    resolution_condition: "the issue is resolved".to_owned(),
                    idempotency_key: format!("concurrent-prior-{round}"),
                },
            )
            .await
            .expect("prior route request");
    }

    let request = |key: &'static str| services::QualityRequestInput {
        source_artifact_id: None,
        category: db::TaskQualityCategory::Specialist,
        target_kind: db::TaskQualityTargetKind::Specialist,
        target_task_role_id: Some(role_id.to_owned()),
        target_actor: None,
        summary: "concurrent route chain".to_owned(),
        findings: Vec::new(),
        resolution_condition: "the issue is resolved".to_owned(),
        idempotency_key: key.to_owned(),
    };
    let (left, right) = tokio::join!(
        fixture.service.create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            request("concurrent-left"),
        ),
        fixture.service.create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            request("concurrent-right"),
        ),
    );
    let left = left.expect("left concurrent request");
    let right = right.expect("right concurrent request");
    assert_eq!(left.route_id, right.route_id);
    let mut rounds = vec![left.route_round, right.route_round];
    rounds.sort_unstable();
    assert_eq!(rounds, vec![4, 5]);
    assert_eq!(
        [left.state, right.state]
            .into_iter()
            .filter(|state| *state == db::TaskQualityState::RequiresHuman)
            .count(),
        1
    );

    let replay = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            request("concurrent-left"),
        )
        .await
        .expect("replay");
    assert_eq!(replay.id, left.id);
    let counters: (i64, i64) = sqlx::query_as(
        "SELECT request_count, budget_used FROM task_quality_route
         WHERE task_id = ? AND route_id = ?",
    )
    .bind(&fixture.task_id)
    .bind(&left.route_id)
    .fetch_one(fixture.db.pool())
    .await
    .expect("route counters");
    assert_eq!(counters, (5, 4));
}

#[tokio::test]
async fn pending_role_ping_pong_is_durably_escalated_with_exact_cycle_evidence() {
    let fixture = fixture("quality-route-ping-pong").await;
    let role_a = "quality-cycle-role-a";
    let role_b = "quality-cycle-role-b";
    add_quality_role(&fixture, role_a, "custom-alpha").await;
    add_quality_role(&fixture, role_b, "custom-beta").await;
    let make_request = |role_id: &str, key: &str| services::QualityRequestInput {
        source_artifact_id: None,
        category: db::TaskQualityCategory::Specialist,
        target_kind: db::TaskQualityTargetKind::Specialist,
        target_task_role_id: Some(role_id.to_owned()),
        target_actor: None,
        summary: "same pending quality chain".to_owned(),
        findings: Vec::new(),
        resolution_condition: "the issue is resolved".to_owned(),
        idempotency_key: key.to_owned(),
    };
    let first = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            make_request(role_a, "cycle-a-1"),
        )
        .await
        .expect("first role route");
    let second = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            make_request(role_b, "cycle-b-2"),
        )
        .await
        .expect("second role route");
    let third = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            make_request(role_a, "cycle-a-3"),
        )
        .await
        .expect("cycle escalation");

    assert_eq!(first.route_id, second.route_id);
    assert_eq!(second.route_id, third.route_id);
    assert_eq!(third.state, db::TaskQualityState::RequiresHuman);
    assert_eq!(
        third.requires_human_reason_code.as_deref(),
        Some("coordination.quality_route_ping_pong")
    );
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&third.cycle_request_ids_json)
            .expect("cycle request IDs"),
        vec![first.id.clone(), second.id.clone(), third.id.clone()]
    );
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&third.cycle_task_role_ids_json)
            .expect("cycle TaskRole IDs"),
        vec![role_a.to_owned(), role_b.to_owned(), role_a.to_owned()]
    );
    let recovered = TaskQualityService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));
    assert_eq!(recovered.process_pending(10).await.expect("recovery"), 2);
    let persisted = db::TaskCoordinationRepo::get_task_quality_request(&*fixture.db, &third.id)
        .await
        .expect("durable escalation read")
        .expect("escalation exists");
    assert_eq!(persisted.state, db::TaskQualityState::RequiresHuman);
    assert_eq!(
        persisted.requires_human_reason_code.as_deref(),
        Some("coordination.quality_route_ping_pong")
    );
}

#[tokio::test]
async fn specialist_quality_request_is_materialized_and_resolved() {
    let fixture = fixture("quality-specialist").await;
    let specialist_role_id = "quality-specialist-role".to_owned();
    let now = now_rfc3339();
    TaskRoleRepo::create_with_configuration(
        &*fixture.db,
        CreateTaskRole {
            id: specialist_role_id.clone(),
            task_id: fixture.task_id.clone(),
            role: "specialist".to_owned(),
            coordination_mode: Some(CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":"Specialist","capabilities":[],"rework":null}"#,
    )
    .await
    .expect("specialist role");
    RoleMembershipRepo::add(
        &*fixture.db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: specialist_role_id.clone(),
            actor_kind: ActorKind::Human,
            actor_id: fixture.user_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("specialist membership");
    let quality = TaskQualityService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));
    let request = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            services::QualityRequestInput {
                source_artifact_id: None,
                category: db::TaskQualityCategory::Specialist,
                target_kind: db::TaskQualityTargetKind::Specialist,
                target_task_role_id: Some(specialist_role_id),
                target_actor: Some(db::ActorRef::Human(fixture.user_id.clone())),
                summary: "Investigate the cache invalidation race".to_owned(),
                findings: vec!["The invalidator can run before the write commits".to_owned()],
                resolution_condition: "The investigation names the exact race and its guard"
                    .to_owned(),
                idempotency_key: "specialist-request".to_owned(),
            },
        )
        .await
        .expect("quality request");
    assert_eq!(request.state, db::TaskQualityState::Notified);
    assert_eq!(
        quality.process_pending(10).await.expect("quality consumer"),
        1
    );
    let request = db::TaskCoordinationRepo::get_task_quality_request(&*fixture.db, &request.id)
        .await
        .expect("request")
        .expect("request row");
    assert_eq!(request.state, db::TaskQualityState::WorkCreated);
    let unit_id = request.work_unit_id.expect("linked WorkUnit");
    let unit = db::WorkUnitRepo::get_by_id(&*fixture.db, &unit_id)
        .await
        .expect("unit")
        .expect("unit row");
    assert_eq!(unit.task_id, fixture.task_id);
    assert_eq!(unit.role, "specialist");
    assert_eq!(unit.status, db::WorkUnitStatus::Open);

    let now = now_rfc3339();
    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, work_unit_id, work_unit_version
         ) VALUES (?, ?, NULL, 'specialist', 'running', ?, ?, 'human', ?, ?, ?)",
    )
    .bind(&execution_id)
    .bind(&fixture.task_id)
    .bind(&now)
    .bind(&now)
    .bind(&fixture.user_id)
    .bind(&unit.id)
    .bind(unit.version)
    .execute(fixture.db.pool())
    .await
    .expect("running execution");
    sqlx::query("UPDATE execution SET status = 'completed', updated_at = ? WHERE id = ?")
        .bind(now_rfc3339())
        .bind(&execution_id)
        .execute(fixture.db.pool())
        .await
        .expect("completed execution");
    quality.process_pending(10).await.expect("reconcile");
    let request = db::TaskCoordinationRepo::get_task_quality_request(&*fixture.db, &request.id)
        .await
        .expect("request")
        .expect("request row");
    assert_eq!(request.state, db::TaskQualityState::Resolved);
    assert_eq!(request.resolution_kind.as_deref(), Some("execution"));
    let unit = db::WorkUnitRepo::get_by_id(&*fixture.db, &unit.id)
        .await
        .expect("unit")
        .expect("unit row");
    assert_eq!(unit.status, db::WorkUnitStatus::Completed);
}

#[tokio::test]
async fn plan_revision_without_a_project_batch_keeps_the_standalone_turn_path() {
    let fixture = fixture("quality-plan-revision-standalone").await;
    let quality = TaskQualityService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));
    let request = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            services::QualityRequestInput {
                source_artifact_id: None,
                category: db::TaskQualityCategory::PlanRevision,
                target_kind: db::TaskQualityTargetKind::Orchestrator,
                target_task_role_id: Some(fixture.role_id.clone()),
                target_actor: None,
                summary: "Revise the Task plan".to_owned(),
                findings: vec!["The plan needs a clearer acceptance condition".to_owned()],
                resolution_condition: "The revised plan states its acceptance condition".to_owned(),
                idempotency_key: "quality-plan-revision-standalone".to_owned(),
            },
        )
        .await
        .expect("quality request");

    assert_eq!(
        quality.process_pending(10).await.expect("quality consumer"),
        1
    );
    let persisted = db::TaskCoordinationRepo::get_task_quality_request(&*fixture.db, &request.id)
        .await
        .expect("quality request read")
        .expect("quality request row");
    assert_eq!(persisted.state, db::TaskQualityState::WorkCreated);
    assert!(persisted.coordination_request_id.is_some());
    let turn_count: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&fixture.task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("standalone turn budget");
    assert_eq!(turn_count, 1);
}

#[tokio::test]
async fn batch_scoped_plan_revision_without_exact_batch_origin_requires_human() {
    let fixture = fixture("quality-plan-revision-batch-scope").await;
    let task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
        .await
        .expect("Task read")
        .expect("Task row");
    let graph_version: i64 =
        sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
            .bind(&task.project_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("Project graph version");
    let project_plan = services::project_plan::ProjectPlanService::new(
        Arc::clone(&fixture.db),
        Arc::new(EventBus::new(32)),
    );
    let batch = project_plan
        .authorize_batch(
            &task.project_id,
            &fixture.user_id,
            graph_version,
            vec![fixture.task_id.clone()],
            1,
            "quality-plan-revision-batch",
        )
        .await
        .expect("owner authorizes the exact Task phase");
    assert_eq!(batch.state, "running");
    let preparation_complete: i64 =
        sqlx::query_scalar("SELECT preparation_complete FROM project_plan_batch WHERE id=?")
            .bind(&batch.id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("batch preparation status persists");
    assert_eq!(preparation_complete, 1);

    let quality = TaskQualityService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));
    let request = fixture
        .service
        .create_quality_request(
            &fixture.task_id,
            &fixture.user_id,
            services::QualityRequestInput {
                source_artifact_id: None,
                category: db::TaskQualityCategory::PlanRevision,
                target_kind: db::TaskQualityTargetKind::Orchestrator,
                target_task_role_id: Some(fixture.role_id.clone()),
                target_actor: None,
                summary: "Revise the Task plan".to_owned(),
                findings: vec!["No source from this authorized batch is attached".to_owned()],
                resolution_condition: "An owner reviews the missing source lineage".to_owned(),
                idempotency_key: "quality-plan-revision-batch-source-missing".to_owned(),
            },
        )
        .await
        .expect("quality request");

    assert_eq!(
        quality.process_pending(10).await.expect("quality consumer"),
        1
    );
    let persisted = db::TaskCoordinationRepo::get_task_quality_request(&*fixture.db, &request.id)
        .await
        .expect("quality request read")
        .expect("quality request row");
    assert_eq!(persisted.state, db::TaskQualityState::RequiresHuman);
    assert_eq!(
        persisted.requires_human_reason_code.as_deref(),
        Some("project_plan_batch_inactive")
    );
    assert!(persisted
        .requires_human_reason
        .as_deref()
        .is_some_and(|reason| reason.contains("exact quality source")));

    let turn_count: i64 =
        sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
            .bind(&fixture.task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("batch turn budget");
    let request_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task_coordination_request WHERE task_id=?")
            .bind(&fixture.task_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("batch coordination requests");
    let execution_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id=?")
        .bind(&fixture.task_id)
        .fetch_one(fixture.db.pool())
        .await
        .expect("batch executions");
    assert_eq!(turn_count, 0);
    assert_eq!(request_count, 0);
    assert_eq!(execution_count, 0);
}

#[tokio::test]
async fn new_project_template_task_derives_custom_review_roles_for_batch_authorization() {
    let fixture = fixture("quality-template-envelope").await;
    let existing_task = TaskRepo::get_by_id(&*fixture.db, &fixture.task_id, false)
        .await
        .expect("existing Task read")
        .expect("existing Task row");
    let coordination =
        db::TaskCoordinationRepo::get_task_coordination(&*fixture.db, &fixture.task_id)
            .await
            .expect("existing Task coordination read")
            .expect("existing Task coordination row");
    let agent_id = match coordination.coordinator_actor {
        Some(db::ActorRef::Agent(id)) => id,
        other => panic!("fixture has no Agent coordinator hint: {other:?}"),
    };
    let defaults = ProjectTaskRoleDefaultsRepo::get_task_role_defaults(
        &*fixture.db,
        &existing_task.project_id,
    )
    .await
    .expect("Project role defaults read");
    let now = now_rfc3339();
    ProjectTaskRoleDefaultsRepo::replace_task_role_defaults(
        &*fixture.db,
        &existing_task.project_id,
        &fixture.user_id,
        defaults.version,
        vec![
            ProjectTaskRoleDefault {
                role: "planner".to_owned(),
                coordination_mode: CoordinationMode::Collaborative,
                configuration_json: serde_json::json!({
                    "schema_version": 1,
                    "display_label": "Planner",
                    "capabilities": ["orchestration"],
                    "rework": null,
                    "coordinator_actor": {"kind": "agent", "id": agent_id.clone()}
                })
                .to_string(),
                members: vec![db::ActorRef::Agent(agent_id.clone())],
            },
            ProjectTaskRoleDefault {
                role: "quality_reviewer".to_owned(),
                coordination_mode: CoordinationMode::Independent,
                configuration_json: serde_json::json!({
                    "schema_version": 1,
                    "display_label": "Quality reviewer",
                    "capabilities": ["formal_review"],
                    "rework": null,
                    "coordinator_actor": null
                })
                .to_string(),
                members: vec![db::ActorRef::Human(fixture.user_id.clone())],
            },
        ],
        &now,
    )
    .await
    .expect("Project template updates to the custom reviewer role");

    let task_service = TaskService::new(Arc::clone(&fixture.db), Arc::new(EventBus::new(32)));
    let task = task_service
        .create_task(
            &existing_task.project_id,
            "Task with custom reviewer template",
            Some("Use the Project's copied coordinator and reviewer roles".to_owned()),
            None,
            None,
            Some(db::TaskLifecycleState::Ready),
            None,
        )
        .await
        .expect("Task copies current Project defaults");
    let initial_coordination =
        db::TaskCoordinationRepo::get_task_coordination(&*fixture.db, &task.id)
            .await
            .expect("new Task coordination read")
            .expect("new Task coordination row");
    assert_eq!(initial_coordination.mode, TaskCoordinationMode::Manual);
    assert!(initial_coordination.requested_by.is_none());
    assert!(initial_coordination.team_stale);
    assert_eq!(
        initial_coordination.coordinator_actor,
        Some(db::ActorRef::Agent(agent_id.clone()))
    );

    let graph_version: i64 =
        sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
            .bind(&existing_task.project_id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("current Project graph version");
    let project_plan = services::project_plan::ProjectPlanService::new(
        Arc::clone(&fixture.db),
        Arc::new(EventBus::new(32)),
    );
    project_plan
        .authorize_batch(
            &existing_task.project_id,
            &fixture.user_id,
            graph_version,
            vec![task.id.clone()],
            1,
            "template-custom-reviewer-batch",
        )
        .await
        .expect("batch prepares the Task from the exact Project template");

    let configured: String =
        sqlx::query_scalar("SELECT envelope_json FROM task_coordination WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(fixture.db.pool())
            .await
            .expect("batch authorization envelope");
    let configured: serde_json::Value =
        serde_json::from_str(&configured).expect("stored envelope is valid");
    assert_eq!(
        configured["allowed_review_roles"],
        serde_json::json!(["quality_reviewer"])
    );
    assert_eq!(configured["max_work_units"], 16);
    assert!(configured["require_independent_review"]
        .as_bool()
        .unwrap_or(false));
}
