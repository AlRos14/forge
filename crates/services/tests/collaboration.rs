use std::sync::Arc;

use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind, AgentRepo,
    AgentStatus, ArtifactKind, ArtifactStorageKind, CollaborationRepo, CollaborationTarget,
    CoordinationMode, CreateAgentIdentity, CreateAgentProfile, CreateArtifact, CreateDomainEvent,
    CreateRoleMembership, CreateTaskRole, DecisionOutcome, DomainEventRepo, HandoffStatus,
    ProjectRepo, ProposalTarget, ProposalTargetKind, RoleMembershipRepo, RoleMembershipStatus,
    SqliteDb, TaskRoleRepo,
};
use events::EventBus;
use services::{
    CollaborationActorSource, CollaborationService, CreateArtifactInput, CreateDecisionInput,
    CreateHandoffInput, CreateMessageInput, CreateProposalInput,
};

struct Fixture {
    db: Arc<SqliteDb>,
    service: CollaborationService,
    project_id: String,
    task_id: String,
    second_task_id: String,
    user_id: String,
    execution_id: String,
}

async fn fixture() -> Fixture {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let db = Arc::new(SqliteDb::new(pool));
    let now = now_rfc3339();
    let project_id = "pr4-project".to_owned();
    let repo_id = "pr4-repo".to_owned();
    let task_id = "pr4-task".to_owned();
    let second_task_id = "pr4-task-2".to_owned();
    let user_id = "pr4-human".to_owned();
    let execution_id = "pr4-human-execution".to_owned();

    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES (?, 'pr4@example.test', 'test', ?, ?)",
    )
    .bind(&user_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("user");
    sqlx::query(
        "INSERT INTO project (id, name, settings, created_at, updated_at)
         VALUES (?, 'PR4', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("project");
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at)
         VALUES (?, ?, 'test', 'https://example.invalid/pr4.git', NULL, 'direct_merge', 'main', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("repo");
    for task_id in [&task_id, &second_task_id] {
        sqlx::query(
            "INSERT INTO task (id, project_id, repo_id, title, task_type, status, created_at, updated_at)
             VALUES (?, ?, ?, 'PR4 Task', 'implementation', 'in_progress', ?, ?)",
        )
        .bind(task_id)
        .bind(&project_id)
        .bind(&repo_id)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("task");
    }
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, purpose
         ) VALUES (?, ?, NULL, 'interactive', 'completed', ?, ?, 'human', ?, 'general')",
    )
    .bind(&execution_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .bind(&user_id)
    .execute(db.pool())
    .await
    .expect("Human Execution");

    Fixture {
        service: CollaborationService::new(Arc::clone(&db), Arc::new(EventBus::new(32))),
        db,
        project_id,
        task_id,
        second_task_id,
        user_id,
        execution_id,
    }
}

async fn add_agent_execution(f: &Fixture) -> (String, String, String) {
    let agent_id = "pr4-agent".to_owned();
    let profile_id = "pr4-agent-profile".to_owned();
    let role_id = "pr4-implementer-role".to_owned();
    let execution_id = "pr4-agent-execution".to_owned();
    let now = now_rfc3339();
    AgentRepo::create_identity_with_profile(
        &*f.db,
        CreateAgentIdentity {
            id: agent_id.clone(),
            name: "PR4 Agent".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(f.user_id.clone()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: profile_id.clone(),
            identity_id: agent_id.clone(),
            backend_kind: "native".to_owned(),
            executor_type: "test".to_owned(),
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
    .expect("Agent identity");
    TaskRoleRepo::create(
        &*f.db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: f.task_id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: Some(CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("TaskRole");
    RoleMembershipRepo::add(
        &*f.db,
        CreateRoleMembership {
            id: "pr4-agent-membership".to_owned(),
            task_role_id: role_id.clone(),
            actor_kind: ActorKind::Agent,
            actor_id: agent_id.clone(),
            status: RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("active membership");
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, purpose
         ) VALUES (?, ?, ?, 'coder', 'completed', ?, ?, 'agent', ?, 'implement')",
    )
    .bind(&execution_id)
    .bind(&f.task_id)
    .bind(&agent_id)
    .bind(&now)
    .bind(&now)
    .bind(&agent_id)
    .execute(f.db.pool())
    .await
    .expect("Agent Execution");
    (agent_id, role_id, execution_id)
}

fn human(fixture: &Fixture) -> CollaborationActorSource {
    CollaborationActorSource::Human(fixture.user_id.clone())
}

#[tokio::test]
async fn generic_collaboration_is_scoped_immutable_evented_and_teardown_safe() {
    let f = fixture().await;
    let actor = human(&f);
    let artifact = f
        .service
        .create_artifact(
            actor.clone(),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: f.execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some("artifact-secret".to_owned()),
                content_ref: None,
                metadata_json: r#"{"safe":true}"#.to_owned(),
                digest: Some("sha256:test".to_owned()),
            },
        )
        .await
        .expect("inline Artifact");
    assert_eq!(artifact.producer, db::ActorRef::Human(f.user_id.clone()));
    assert_eq!(artifact.producer_execution_id, f.execution_id);
    let artifact_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event WHERE entity_type = 'artifact' AND entity_id = ?",
    )
    .bind(&artifact.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Artifact event");
    assert!(!artifact_event.contains("artifact-secret"));
    assert!(!artifact_event.contains("content_ref"));

    let actor_column: Option<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('artifact') WHERE name = 'actor_id'",
    )
    .fetch_optional(f.db.pool())
    .await
    .expect("Artifact columns");
    assert!(actor_column.is_none());
    let immutable = sqlx::query("UPDATE artifact SET content = 'changed' WHERE id = ?")
        .bind(&artifact.id)
        .execute(f.db.pool())
        .await
        .expect_err("Artifact immutable");
    assert!(immutable.to_string().contains("immutable"));

    let message = f
        .service
        .create_message(
            actor.clone(),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                target: CollaborationTarget::Task,
                body: "message-secret".to_owned(),
                artifact_ids: vec![artifact.id.clone()],
            },
        )
        .await
        .expect("Message");
    assert_eq!(message.artifact_ids, vec![artifact.id.clone()]);
    let message_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event WHERE entity_type = 'message' AND entity_id = ?",
    )
    .bind(&message.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Message event");
    assert!(!message_event.contains("message-secret"));
    let message_immutable = sqlx::query("UPDATE message SET body = 'changed' WHERE id = ?")
        .bind(&message.id)
        .execute(f.db.pool())
        .await
        .expect_err("Message immutable");
    assert!(message_immutable.to_string().contains("immutable"));

    let cross_task = f
        .service
        .create_message(
            actor.clone(),
            CreateMessageInput {
                task_id: f.second_task_id.clone(),
                target: CollaborationTarget::Task,
                body: "cross-task".to_owned(),
                artifact_ids: vec![artifact.id.clone()],
            },
        )
        .await
        .expect_err("cross-Task Artifact link is rejected");
    assert!(matches!(
        cross_task,
        services::ServiceError::NotFound { .. }
    ));
    let foreign_role = TaskRoleRepo::create(
        &*f.db,
        CreateTaskRole {
            id: "pr4-other-role".to_owned(),
            task_id: f.second_task_id.clone(),
            role: "reviewer".to_owned(),
            coordination_mode: Some(CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("second Task Role");
    let cross_task_role = f
        .service
        .create_message(
            actor.clone(),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                target: CollaborationTarget::Role(foreign_role.id),
                body: "cross-task role target".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect_err("cross-Task Role target is rejected");
    assert!(matches!(
        cross_task_role,
        services::ServiceError::NotFound { .. }
    ));

    let handoff = f
        .service
        .create_handoff(
            actor.clone(),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                source_role_id: None,
                target: CollaborationTarget::Task,
                intent: db::HandoffIntent::Question,
                parent_execution_id: Some(f.execution_id.clone()),
                expected_policy_ref: Some("policy://review".to_owned()),
                artifact_ids: vec![artifact.id.clone()],
            },
        )
        .await
        .expect("Handoff");
    let accepted = f
        .service
        .transition_handoff(
            actor.clone(),
            &handoff.id,
            handoff.version,
            HandoffStatus::Accepted,
        )
        .await
        .expect("Handoff transition");
    assert_eq!(accepted.version, 2);
    let invalid_transition = f
        .service
        .transition_handoff(
            actor.clone(),
            &handoff.id,
            accepted.version,
            HandoffStatus::Pending,
        )
        .await
        .expect_err("invalid lifecycle transition rejected");
    assert!(matches!(
        invalid_transition,
        services::ServiceError::InvalidOperation { .. }
    ));
    let handoff_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event
         WHERE entity_type = 'handoff' AND entity_id = ? AND event_type = 'handoff.created'",
    )
    .bind(&handoff.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Handoff event");
    assert!(!handoff_event.contains("policy://review"));
    let memberships: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM role_membership")
        .fetch_one(f.db.pool())
        .await
        .expect("membership count");
    assert_eq!(memberships, 0, "Handoff never assigns a Role");

    let proposal = f
        .service
        .create_proposal(
            actor.clone(),
            CreateProposalInput {
                task_id: f.task_id.clone(),
                target: ProposalTarget {
                    kind: ProposalTargetKind::Task,
                    id: f.task_id.clone(),
                },
                action: "ship-change".to_owned(),
                reason: "proposal-secret".to_owned(),
                target_version: None,
                target_digest: None,
                required_policy_ref: Some("policy://release".to_owned()),
                required_policy_version: Some(3),
                required_policy_digest: Some("sha256:policy".to_owned()),
                supersedes_proposal_id: None,
                artifact_ids: vec![artifact.id.clone()],
            },
        )
        .await
        .expect("Proposal");
    let no_deciders = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.task_id.clone(),
                proposal_id: proposal.id.clone(),
                proposal_version: proposal.content_version,
                outcome: DecisionOutcome::Approve,
                rationale: "must fail without a decider".to_owned(),
                policy_ref: None,
                policy_version: None,
                policy_digest: None,
            },
            vec![],
        )
        .await
        .expect_err("Decision requires a decider");
    assert!(matches!(
        no_deciders,
        services::ServiceError::InvalidOperation { .. }
    ));
    let cross_task_decision = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.second_task_id.clone(),
                proposal_id: proposal.id.clone(),
                proposal_version: proposal.content_version,
                outcome: DecisionOutcome::Approve,
                rationale: "cross-task proposal".to_owned(),
                policy_ref: None,
                policy_version: None,
                policy_digest: None,
            },
            vec![human(&f)],
        )
        .await
        .expect_err("Decision cannot target another Task's Proposal");
    assert!(matches!(
        cross_task_decision,
        services::ServiceError::InvalidOperation { .. }
    ));
    let proposal_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event WHERE entity_type = 'proposal' AND entity_id = ?",
    )
    .bind(&proposal.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Proposal event");
    assert!(!proposal_event.contains("proposal-secret"));
    assert!(!proposal_event.contains("policy://release"));
    let decision = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.task_id.clone(),
                proposal_id: proposal.id.clone(),
                proposal_version: proposal.content_version,
                outcome: DecisionOutcome::Approve,
                rationale: "decision-secret".to_owned(),
                policy_ref: Some("policy://release".to_owned()),
                policy_version: Some(3),
                policy_digest: Some("sha256:policy".to_owned()),
            },
            vec![actor],
        )
        .await
        .expect("Decision");
    assert_eq!(decision.actors.len(), 1);
    let decision_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event WHERE entity_type = 'decision' AND entity_id = ?",
    )
    .bind(&decision.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Decision event");
    assert!(!decision_event.contains("decision-secret"));
    assert!(!decision_event.contains("policy://release"));
    let proposal_immutable = sqlx::query("UPDATE proposal SET action = 'changed' WHERE id = ?")
        .bind(&proposal.id)
        .execute(f.db.pool())
        .await
        .expect_err("Proposal content immutable");
    assert!(proposal_immutable.to_string().contains("immutable"));
    let decision_immutable = sqlx::query("UPDATE decision SET rationale = 'changed' WHERE id = ?")
        .bind(&decision.id)
        .execute(f.db.pool())
        .await
        .expect_err("Decision immutable");
    assert!(decision_immutable.to_string().contains("immutable"));

    for legacy_table in [
        "task_plan_revision",
        "agent_chat_message",
        "agent_handoff",
        "project_decision",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {legacy_table}"))
            .fetch_one(f.db.pool())
            .await
            .expect("legacy table exists");
        assert_eq!(count, 0, "generic writers do not dual-write {legacy_table}");
    }

    ProjectRepo::delete(&*f.db, &f.project_id)
        .await
        .expect("official Project teardown");
    for table in [
        "artifact",
        "artifact_execution_producer",
        "message",
        "message_artifact",
        "handoff",
        "handoff_artifact",
        "proposal",
        "proposal_artifact",
        "decision",
        "decision_actor",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(f.db.pool())
            .await
            .expect("PR4 table remains queryable");
        assert_eq!(count, 0, "Project teardown clears {table}");
    }
    let foreign_key_issues: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(f.db.pool())
            .await
            .expect("FK check");
    assert!(foreign_key_issues.is_empty());
}

#[tokio::test]
async fn artifact_requires_valid_storage_and_same_task_execution_producer() {
    let f = fixture().await;
    let invalid_xor = f
        .service
        .create_artifact(
            human(&f),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: f.execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::External,
                content: Some("content".to_owned()),
                content_ref: Some("internal://ref".to_owned()),
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect_err("storage XOR is enforced");
    assert!(matches!(
        invalid_xor,
        services::ServiceError::InvalidOperation { .. }
    ));

    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, purpose
         ) VALUES ('pr4-other-execution', ?, NULL, 'interactive', 'completed', ?, ?, 'human', ?, 'general')",
    )
    .bind(&f.second_task_id)
    .bind(&now)
    .bind(&now)
    .bind(&f.user_id)
    .execute(f.db.pool())
    .await
    .expect("second Task Execution");
    let cross_task = f
        .service
        .create_artifact(
            human(&f),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: "pr4-other-execution".to_owned(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some("safe".to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect_err("cross-Task producer is rejected");
    assert!(matches!(
        cross_task,
        services::ServiceError::NotFound { .. }
    ));

    let no_producer = sqlx::query(
        "INSERT INTO artifact (
             id, task_id, kind, storage_kind, content, content_ref, metadata_json, digest, created_at
         ) VALUES ('pr4-orphan', ?, 'summary', 'inline', 'orphan', NULL, '{}', NULL, ?)",
    )
    .bind(&f.task_id)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect_err("Artifact cannot be inserted without producer");
    assert!(no_producer.to_string().contains("producer"));

    let _ = CollaborationRepo::get_artifact(&*f.db, "missing")
        .await
        .expect("missing Artifact is not corrupt");

    sqlx::query("DROP TRIGGER artifact_producer_required_insert")
        .execute(f.db.pool())
        .await
        .expect("disable insert guard to model corrupted persisted data");
    sqlx::query(
        "INSERT INTO artifact (
             id, task_id, kind, storage_kind, content, content_ref, metadata_json, digest, created_at
         ) VALUES ('pr4-corrupt-orphan', ?, 'summary', 'inline', 'orphan', NULL, '{}', NULL, ?)",
    )
    .bind(&f.task_id)
    .bind(now_rfc3339())
    .execute(f.db.pool())
    .await
    .expect("fixture creates a corrupt legacy-like row");
    let corrupt = CollaborationRepo::get_artifact(&*f.db, "pr4-corrupt-orphan")
        .await
        .expect_err("reader fails closed on missing producer");
    assert!(corrupt.to_string().contains("producer"));
}

#[tokio::test]
async fn agent_identity_is_derived_from_execution_and_decisions_support_mixed_deciders() {
    let f = fixture().await;
    let (agent_id, role_id, execution_id) = add_agent_execution(&f).await;
    let agent_source = CollaborationActorSource::Execution(execution_id.clone());
    let execution_count_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution")
        .fetch_one(f.db.pool())
        .await
        .expect("execution count");
    let message = f
        .service
        .create_message(
            agent_source.clone(),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                target: CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone())),
                body: "Agent-authored communication".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Agent Message");
    assert_eq!(message.sender, db::ActorRef::Agent(agent_id.clone()));
    let read_message = f
        .service
        .get_message(&message.id, agent_source.clone())
        .await
        .expect("Agent reads authorized same-Task Message");
    assert_eq!(read_message.body, "Agent-authored communication");
    let role_message = f
        .service
        .create_message(
            agent_source.clone(),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                target: CollaborationTarget::Role(role_id.clone()),
                body: "Role-addressed communication".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Role-addressed Message");
    assert_eq!(
        role_message.target,
        CollaborationTarget::Role(role_id.clone())
    );
    let agent_message_page = f
        .service
        .list_messages(
            &f.task_id,
            agent_source.clone(),
            db::PageRequest {
                cursor: None,
                limit: 10,
                include_total: false,
                sort_by: db::SortBy::CreatedAt,
                sort_order: db::SortOrder::Desc,
            },
        )
        .await
        .expect("Agent lists authorized same-Task Messages");
    assert_eq!(agent_message_page.items.len(), 2);

    let artifact = f
        .service
        .create_artifact(
            agent_source.clone(),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::External,
                content: None,
                content_ref: Some("private://agent-output".to_owned()),
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect("Agent Artifact");
    assert_eq!(artifact.producer, db::ActorRef::Agent(agent_id.clone()));
    assert_eq!(
        f.service
            .get_artifact(&artifact.id, agent_source.clone())
            .await
            .expect("Agent reads its authorized Artifact")
            .producer,
        db::ActorRef::Agent(agent_id.clone())
    );
    assert_eq!(
        f.service
            .list_artifacts(
                &f.task_id,
                agent_source.clone(),
                db::PageRequest {
                    cursor: None,
                    limit: 10,
                    include_total: false,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Desc,
                },
            )
            .await
            .expect("Agent lists authorized Artifacts")
            .items
            .len(),
        1
    );

    let memberships_before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM role_membership")
        .fetch_one(f.db.pool())
        .await
        .expect("membership count");
    let handoff = f
        .service
        .create_handoff(
            agent_source.clone(),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                source_role_id: None,
                target: CollaborationTarget::Role(role_id.clone()),
                intent: db::HandoffIntent::Delegation,
                parent_execution_id: None,
                expected_policy_ref: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Agent Handoff");
    assert_eq!(handoff.created_by, db::ActorRef::Agent(agent_id.clone()));
    assert_eq!(
        f.service
            .get_handoff(&handoff.id, agent_source.clone())
            .await
            .expect("Agent reads authorized Handoff")
            .created_by,
        db::ActorRef::Agent(agent_id.clone())
    );
    assert_eq!(
        f.service
            .list_handoffs(
                &f.task_id,
                agent_source.clone(),
                db::PageRequest {
                    cursor: None,
                    limit: 10,
                    include_total: false,
                    sort_by: db::SortBy::CreatedAt,
                    sort_order: db::SortOrder::Desc,
                },
            )
            .await
            .expect("Agent lists authorized Handoffs")
            .items
            .len(),
        1
    );
    let actor_handoff = f
        .service
        .create_handoff(
            agent_source.clone(),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                source_role_id: Some(role_id.clone()),
                target: CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone())),
                intent: db::HandoffIntent::Question,
                parent_execution_id: Some(execution_id.clone()),
                expected_policy_ref: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Actor-addressed Handoff");
    assert_eq!(
        actor_handoff.target,
        CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone()))
    );
    let memberships_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM role_membership")
        .fetch_one(f.db.pool())
        .await
        .expect("membership count");
    assert_eq!(memberships_before, memberships_after);

    let proposal = f
        .service
        .create_proposal(
            agent_source.clone(),
            CreateProposalInput {
                task_id: f.task_id.clone(),
                target: ProposalTarget {
                    kind: ProposalTargetKind::Execution,
                    id: execution_id.clone(),
                },
                action: "change-worker-plan".to_owned(),
                reason: "Agent proposal".to_owned(),
                target_version: None,
                target_digest: None,
                required_policy_ref: Some("policy://task".to_owned()),
                required_policy_version: None,
                required_policy_digest: None,
                supersedes_proposal_id: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Agent Proposal");
    assert_eq!(proposal.proposer, db::ActorRef::Agent(agent_id.clone()));
    assert_eq!(
        f.service
            .get_proposal(&proposal.id, agent_source.clone())
            .await
            .expect("Agent reads authorized Proposal")
            .proposer,
        db::ActorRef::Agent(agent_id.clone())
    );
    let decision = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.task_id.clone(),
                proposal_id: proposal.id.clone(),
                proposal_version: proposal.content_version,
                outcome: DecisionOutcome::Supersede,
                rationale: "Replace this intent with a new Proposal".to_owned(),
                policy_ref: None,
                policy_version: None,
                policy_digest: None,
            },
            vec![
                human(&f),
                CollaborationActorSource::Execution(execution_id.clone()),
            ],
        )
        .await
        .expect("mixed deciders");
    assert_eq!(decision.actors.len(), 2);
    assert!(decision.actors.contains(&db::ActorRef::Human(f.user_id)));
    assert!(decision.actors.contains(&db::ActorRef::Agent(agent_id)));
    assert_eq!(
        f.service
            .get_decision(&decision.id, agent_source.clone())
            .await
            .expect("Agent reads authorized Decision")
            .actors
            .len(),
        2
    );
    let superseding = f
        .service
        .create_proposal(
            agent_source,
            CreateProposalInput {
                task_id: f.task_id.clone(),
                target: ProposalTarget {
                    kind: ProposalTargetKind::Execution,
                    id: execution_id,
                },
                action: "revised-worker-plan".to_owned(),
                reason: "Supersedes the prior intent".to_owned(),
                target_version: None,
                target_digest: None,
                required_policy_ref: None,
                required_policy_version: None,
                required_policy_digest: None,
                supersedes_proposal_id: Some(proposal.id.clone()),
                artifact_ids: vec![],
            },
        )
        .await
        .expect("new Proposal follows supersede Decision");
    assert_eq!(superseding.supersedes_proposal_id, Some(proposal.id));
    let execution_count_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM execution")
        .fetch_one(f.db.pool())
        .await
        .expect("execution count");
    assert_eq!(execution_count_before, execution_count_after);
}

#[tokio::test]
async fn database_rejects_unknown_and_system_actor_refs() {
    let f = fixture().await;
    let now = now_rfc3339();
    for (kind, id) in [("human", "missing-human"), ("agent", "missing-agent")] {
        let error = sqlx::query(
            "INSERT INTO message (
                 id, task_id, sender_actor_kind, sender_actor_id,
                 target_kind, target_actor_kind, target_actor_id, target_role_id, body, created_at
             ) VALUES (?, ?, ?, ?, 'task', NULL, NULL, NULL, 'body', ?)",
        )
        .bind(format!("message-{kind}"))
        .bind(&f.task_id)
        .bind(kind)
        .bind(id)
        .bind(&now)
        .execute(f.db.pool())
        .await
        .expect_err("missing ActorRef is rejected by DB guard");
        assert!(error.to_string().contains("ActorRef"));
    }
    let system = sqlx::query(
        "INSERT INTO message (
             id, task_id, sender_actor_kind, sender_actor_id,
             target_kind, target_actor_kind, target_actor_id, target_role_id, body, created_at
         ) VALUES ('message-system', ?, 'system', 'system', 'task', NULL, NULL, NULL, 'body', ?)",
    )
    .bind(&f.task_id)
    .bind(now)
    .execute(f.db.pool())
    .await
    .expect_err("System Actor is rejected by DB check");
    assert!(system.to_string().contains("CHECK constraint failed"));
}

#[tokio::test]
async fn artifact_and_domain_event_rollback_together() {
    let f = fixture().await;
    let event_id = new_uuid_v4();
    let now = now_rfc3339();
    let seed_event = CreateDomainEvent {
        id: event_id.clone(),
        event_type: "test.existing".to_owned(),
        entity_type: "test".to_owned(),
        entity_id: "existing".to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(f.user_id.clone()),
        scope_type: "task".to_owned(),
        scope_id: f.task_id.clone(),
        correlation_id: new_uuid_v4(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: "{}".to_owned(),
        created_at: now.clone(),
    };
    DomainEventRepo::append_event(&*f.db, seed_event.clone())
        .await
        .expect("seed event");

    let artifact_id = new_uuid_v4();
    let error = CollaborationRepo::create_artifact(
        &*f.db,
        CreateArtifact {
            id: artifact_id.clone(),
            task_id: f.task_id.clone(),
            kind: ArtifactKind::Summary,
            storage_kind: ArtifactStorageKind::Inline,
            content: Some("must roll back".to_owned()),
            content_ref: None,
            metadata_json: "{}".to_owned(),
            digest: None,
            producer_execution_id: f.execution_id.clone(),
            created_at: now,
        },
        CreateDomainEvent {
            id: event_id.clone(),
            event_type: "artifact.created".to_owned(),
            entity_type: "artifact".to_owned(),
            entity_id: artifact_id.clone(),
            actor_type: "human".to_owned(),
            actor_id: Some(f.user_id.clone()),
            scope_type: "task".to_owned(),
            scope_id: f.task_id.clone(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: "{}".to_owned(),
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect_err("duplicate event id rolls back the write");
    assert!(matches!(error, db::DbError::Sqlx(_)));

    let artifact_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE id = ?")
        .bind(&artifact_id)
        .fetch_one(f.db.pool())
        .await
        .expect("Artifact count");
    let producer_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM artifact_execution_producer WHERE artifact_id = ?",
    )
    .bind(&artifact_id)
    .fetch_one(f.db.pool())
    .await
    .expect("producer count");
    let event_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM domain_event WHERE id = ?")
        .bind(&event_id)
        .fetch_one(f.db.pool())
        .await
        .expect("event count");
    assert_eq!(artifact_count, 0);
    assert_eq!(producer_count, 0);
    assert_eq!(event_count, 1, "the pre-existing event remains unchanged");
}
