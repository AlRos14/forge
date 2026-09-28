use std::sync::Arc;

use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, ActorKind,
    AgentChatMessageAuthorType, AgentChatMessageRepo, AgentChatMessageStatus, AgentChatRepo,
    AgentHandoffRepo, AgentRepo, AgentStatus, ArtifactKind, ArtifactStorageKind, CollaborationRepo,
    CollaborationTarget, CoordinationMode, CreateAgentChatMessage, CreateAgentHandoff,
    CreateAgentIdentity, CreateAgentProfile, CreateArtifact, CreateDomainEvent,
    CreateProjectDecision, CreateRoleMembership, CreateTaskRole, CreateWorkspace, DecisionOutcome,
    DomainEventRepo, HandoffIntent, HandoffStatus, ProjectOrchestrationRepo, ProjectRepo,
    ProposalTarget, ProposalTargetKind, RoleMembershipRepo, RoleMembershipStatus, SqliteDb,
    TaskRoleRepo, UserRepo, WorkspaceRepo, WorkspaceStatus,
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
    repo_id: String,
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
        repo_id,
        task_id,
        second_task_id,
        user_id,
        execution_id,
    }
}

struct ForeignProjectFixture {
    project_id: String,
    repo_id: String,
    task_id: String,
    user_id: String,
    execution_id: String,
}

async fn create_foreign_project(f: &Fixture) -> ForeignProjectFixture {
    let now = now_rfc3339();
    let project_id = "pr4-project-b".to_owned();
    let repo_id = "pr4-repo-b".to_owned();
    let task_id = "pr4-task-b".to_owned();
    let user_id = "pr4-human-b".to_owned();
    let execution_id = "pr4-human-execution-b".to_owned();

    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES (?, 'pr4-b@example.test', 'test', ?, ?)",
    )
    .bind(&user_id)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("Project B owner");
    sqlx::query(
        "INSERT INTO project (id, name, settings, owner_id, created_at, updated_at)
         VALUES (?, 'PR4 Project B', '{}', ?, ?, ?)",
    )
    .bind(&project_id)
    .bind(&user_id)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("independent Project B");
    sqlx::query(
        "INSERT INTO repo (
             id, project_id, name, remote_url, local_path, work_mode,
             default_branch, created_at, updated_at
         ) VALUES (?, ?, 'test-b', 'https://example.invalid/pr4-b.git', NULL,
                   'direct_merge', 'main', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("Project B repo");
    sqlx::query(
        "INSERT INTO task (
             id, project_id, repo_id, title, task_type, status, created_at, updated_at
         ) VALUES (?, ?, ?, 'Project B Task', 'implementation', 'in_progress', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("Project B Task");
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
    .execute(f.db.pool())
    .await
    .expect("Project B Human Execution");

    ForeignProjectFixture {
        project_id,
        repo_id,
        task_id,
        user_id,
        execution_id,
    }
}

fn task_proposal(task_id: &str) -> CreateProposalInput {
    CreateProposalInput {
        task_id: task_id.to_owned(),
        target: ProposalTarget {
            kind: ProposalTargetKind::Task,
            id: task_id.to_owned(),
        },
        action: "record-only".to_owned(),
        reason: "scope reference test".to_owned(),
        target_version: None,
        target_digest: None,
        required_policy_ref: None,
        required_policy_version: None,
        required_policy_digest: None,
        supersedes_proposal_id: None,
        artifact_ids: vec![],
    }
}

fn decision_input(
    task_id: &str,
    proposal_id: &str,
    proposal_version: i64,
    outcome: DecisionOutcome,
) -> CreateDecisionInput {
    CreateDecisionInput {
        task_id: task_id.to_owned(),
        proposal_id: proposal_id.to_owned(),
        proposal_version,
        outcome,
        rationale: "scope reference test".to_owned(),
        policy_ref: None,
        policy_version: None,
        policy_digest: None,
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

async fn create_work_unit_reference(fixture: &Fixture, task_id: &str, work_unit_id: &str) {
    let now = now_rfc3339();
    TaskRoleRepo::create(
        &*fixture.db,
        CreateTaskRole {
            id: new_uuid_v4(),
            task_id: task_id.to_owned(),
            role: "pr5-context".to_owned(),
            coordination_mode: Some(CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("WorkUnit TaskRole");
    sqlx::query(
        "INSERT INTO work_unit (
             id, task_id, title, scope, role, created_by_kind, created_by_id,
             created_at, updated_at
         ) VALUES (?, ?, 'collaboration context', 'record reference', 'pr5-context',
                   'human', ?, ?, ?)",
    )
    .bind(work_unit_id)
    .bind(task_id)
    .bind(&fixture.user_id)
    .bind(&now)
    .bind(&now)
    .execute(fixture.db.pool())
    .await
    .expect("WorkUnit reference");
}

#[tokio::test]
async fn work_unit_collaboration_references_are_scoped_and_decisions_do_not_dispatch() {
    let f = fixture().await;
    let work_unit_id = "pr5-collaboration-work-unit";
    let other_work_unit_id = "pr5-collaboration-work-unit-other-task";
    create_work_unit_reference(&f, &f.task_id, work_unit_id).await;
    create_work_unit_reference(&f, &f.second_task_id, other_work_unit_id).await;

    let mut proposal_input = task_proposal(&f.task_id);
    proposal_input.target = ProposalTarget {
        kind: ProposalTargetKind::WorkUnit,
        id: work_unit_id.to_owned(),
    };
    let proposal = f
        .service
        .create_proposal(human(&f), proposal_input)
        .await
        .expect("WorkUnit proposal target is admitted");

    let mut cross_task_input = task_proposal(&f.task_id);
    cross_task_input.target = ProposalTarget {
        kind: ProposalTargetKind::WorkUnit,
        id: other_work_unit_id.to_owned(),
    };
    let cross_task_proposal = f
        .service
        .create_proposal(human(&f), cross_task_input)
        .await
        .expect_err("cross-Task WorkUnit Proposal is hidden");
    assert!(matches!(
        cross_task_proposal,
        services::ServiceError::NotFound {
            entity: "work_unit",
            ..
        }
    ));
    let mut missing_work_unit_input = task_proposal(&f.task_id);
    missing_work_unit_input.target = ProposalTarget {
        kind: ProposalTargetKind::WorkUnit,
        id: "pr5-missing-work-unit".to_owned(),
    };
    let missing_work_unit_proposal = f
        .service
        .create_proposal(human(&f), missing_work_unit_input)
        .await
        .expect_err("missing WorkUnit Proposal target is hidden");
    assert!(matches!(
        missing_work_unit_proposal,
        services::ServiceError::NotFound {
            entity: "work_unit",
            ..
        }
    ));

    let message = f
        .service
        .create_message(
            human(&f),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                target: CollaborationTarget::Task,
                work_unit_id: Some(work_unit_id.to_owned()),
                body: "message context stays separate from its Task recipient".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Message can carry WorkUnit context");
    assert_eq!(message.target, CollaborationTarget::Task);
    assert_eq!(message.work_unit_id.as_deref(), Some(work_unit_id));
    let foreign_message = f
        .service
        .create_message(
            human(&f),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                target: CollaborationTarget::Task,
                work_unit_id: Some(other_work_unit_id.to_owned()),
                body: "cross-Task context is hidden".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect_err("Message WorkUnit context is scoped");
    assert!(matches!(
        foreign_message,
        services::ServiceError::NotFound {
            entity: "work_unit",
            ..
        }
    ));

    let handoff = f
        .service
        .create_handoff(
            human(&f),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                source_role_id: None,
                target: CollaborationTarget::Task,
                work_unit_id: Some(work_unit_id.to_owned()),
                intent: HandoffIntent::Question,
                parent_execution_id: Some(f.execution_id.clone()),
                expected_policy_ref: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Handoff can carry WorkUnit context");
    assert_eq!(handoff.target, CollaborationTarget::Task);
    assert_eq!(handoff.work_unit_id.as_deref(), Some(work_unit_id));
    let foreign_handoff = f
        .service
        .create_handoff(
            human(&f),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                source_role_id: None,
                target: CollaborationTarget::Task,
                work_unit_id: Some(other_work_unit_id.to_owned()),
                intent: HandoffIntent::Question,
                parent_execution_id: Some(f.execution_id.clone()),
                expected_policy_ref: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect_err("Handoff WorkUnit context is scoped");
    assert!(matches!(
        foreign_handoff,
        services::ServiceError::NotFound {
            entity: "work_unit",
            ..
        }
    ));

    f.service
        .record_decision(
            decision_input(
                &f.task_id,
                &proposal.id,
                proposal.content_version,
                DecisionOutcome::Approve,
            ),
            vec![human(&f)],
        )
        .await
        .expect("Decision resolves its Proposal only");
    let status: String = sqlx::query_scalar("SELECT status FROM work_unit WHERE id = ?")
        .bind(work_unit_id)
        .fetch_one(f.db.pool())
        .await
        .expect("WorkUnit remains stored");
    let integration_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM work_unit_integration WHERE work_unit_id = ?")
            .bind(work_unit_id)
            .fetch_one(f.db.pool())
            .await
            .expect("integration count");
    assert_eq!(status, "open");
    assert_eq!(integration_count, 0);
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
                digest: Some("private-storage://digest-secret/path".to_owned()),
            },
        )
        .await
        .expect("inline Artifact");
    assert_eq!(artifact.producer, db::ActorRef::Human(f.user_id.clone()));
    assert_eq!(artifact.producer_execution_id, f.execution_id);
    let external_artifact = f
        .service
        .create_artifact(
            actor.clone(),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: f.execution_id.clone(),
                kind: ArtifactKind::TestReport,
                storage_kind: ArtifactStorageKind::External,
                content: None,
                content_ref: Some("private-storage://content-ref-secret/path".to_owned()),
                metadata_json: "{}".to_owned(),
                digest: Some("digest-secret".to_owned()),
            },
        )
        .await
        .expect("external Artifact");
    assert_eq!(
        external_artifact.content_ref.as_deref(),
        Some("private-storage://content-ref-secret/path")
    );
    let artifact_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event WHERE entity_type = 'artifact' AND entity_id = ?",
    )
    .bind(&artifact.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Artifact event");
    assert!(!artifact_event.contains("artifact-secret"));
    assert!(!artifact_event.contains("content_ref"));
    assert!(!artifact_event.contains("private-storage://digest-secret/path"));

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
                work_unit_id: None,
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
                work_unit_id: None,
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
                work_unit_id: None,
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
                work_unit_id: None,
                source_role_id: None,
                target: CollaborationTarget::Task,
                intent: db::HandoffIntent::Question,
                parent_execution_id: Some(f.execution_id.clone()),
                expected_policy_ref: Some("private-policy://handoff-secret".to_owned()),
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
    assert!(!handoff_event.contains("private-policy://handoff-secret"));
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
                action: "/private/path/action-secret".to_owned(),
                reason: "proposal-secret".to_owned(),
                target_version: None,
                target_digest: None,
                required_policy_ref: Some("private-policy://proposal-secret".to_owned()),
                required_policy_version: Some(3),
                required_policy_digest: Some("policy-snapshot-secret".to_owned()),
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
    let foreign = create_foreign_project(&f).await;
    assert_ne!(f.project_id, foreign.project_id);
    let proposal_b = f
        .service
        .create_proposal(
            CollaborationActorSource::Human(foreign.user_id.clone()),
            task_proposal(&foreign.task_id),
        )
        .await
        .expect("Project B principal can create a Proposal in Project B");
    let cross_task_decision = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.task_id.clone(),
                proposal_id: proposal_b.id,
                proposal_version: proposal_b.content_version,
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
        services::ServiceError::NotFound {
            entity: "proposal",
            ..
        }
    ));
    ProjectRepo::delete(&*f.db, &foreign.project_id)
        .await
        .expect("remove the independent Project fixture");
    let proposal_event: String = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event WHERE entity_type = 'proposal' AND entity_id = ?",
    )
    .bind(&proposal.id)
    .fetch_one(f.db.pool())
    .await
    .expect("Proposal event");
    assert!(!proposal_event.contains("proposal-secret"));
    assert!(!proposal_event.contains("/private/path/action-secret"));
    assert!(!proposal_event.contains("private-policy://proposal-secret"));
    assert!(!proposal_event.contains("policy-snapshot-secret"));
    let decision = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.task_id.clone(),
                proposal_id: proposal.id.clone(),
                proposal_version: proposal.content_version,
                outcome: DecisionOutcome::Approve,
                rationale: "decision-secret".to_owned(),
                policy_ref: Some("private-policy://decision-secret".to_owned()),
                policy_version: Some(3),
                policy_digest: Some("decision-policy-snapshot-secret".to_owned()),
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
    assert!(!decision_event.contains("private-policy://decision-secret"));
    assert!(!decision_event.contains("decision-policy-snapshot-secret"));

    let event_payloads: Vec<String> = sqlx::query_scalar(
        "SELECT payload_json FROM domain_event
         WHERE event_type IN (
             'artifact.created', 'message.created', 'handoff.created',
             'handoff.status_changed', 'proposal.created', 'proposal.withdrawn',
             'decision.recorded'
         )",
    )
    .fetch_all(f.db.pool())
    .await
    .expect("PR4 payloads");
    for sensitive in [
        "artifact-secret",
        "private-storage://digest-secret/path",
        "private-storage://content-ref-secret/path",
        "message-secret",
        "private-policy://handoff-secret",
        "/private/path/action-secret",
        "proposal-secret",
        "private-policy://proposal-secret",
        "policy-snapshot-secret",
        "decision-secret",
        "private-policy://decision-secret",
        "decision-policy-snapshot-secret",
    ] {
        assert!(
            event_payloads
                .iter()
                .all(|payload| !payload.contains(sensitive)),
            "PR4 domain events must not contain {sensitive}"
        );
    }
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
async fn caller_supplied_collaboration_references_are_scoped_before_semantic_load() {
    let f = fixture().await;
    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(&f.user_id)
        .bind(&f.project_id)
        .execute(f.db.pool())
        .await
        .expect("Project A owner");
    let foreign = create_foreign_project(&f).await;
    assert_ne!(f.project_id, foreign.project_id);

    let unauthorized_project_b = f
        .service
        .create_proposal(human(&f), task_proposal(&foreign.task_id))
        .await
        .expect_err("User A cannot authorize Project B");
    assert!(matches!(
        unauthorized_project_b,
        services::ServiceError::NotFound { entity: "task", .. }
    ));

    let proposal_a = f
        .service
        .create_proposal(human(&f), task_proposal(&f.task_id))
        .await
        .expect("Project A Proposal");
    let proposal_b_open = f
        .service
        .create_proposal(
            CollaborationActorSource::Human(foreign.user_id.clone()),
            task_proposal(&foreign.task_id),
        )
        .await
        .expect("Project B principal creates an open Proposal");
    let proposal_b_resolved = f
        .service
        .create_proposal(
            CollaborationActorSource::Human(foreign.user_id.clone()),
            task_proposal(&foreign.task_id),
        )
        .await
        .expect("Project B principal creates a resolvable Proposal");
    f.service
        .record_decision(
            decision_input(
                &foreign.task_id,
                &proposal_b_resolved.id,
                proposal_b_resolved.content_version,
                DecisionOutcome::Approve,
            ),
            vec![CollaborationActorSource::Human(foreign.user_id.clone())],
        )
        .await
        .expect("Project B principal resolves its Proposal");
    let proposal_b_corrupt = f
        .service
        .create_proposal(
            CollaborationActorSource::Human(foreign.user_id.clone()),
            task_proposal(&foreign.task_id),
        )
        .await
        .expect("Project B principal creates a Proposal to corrupt");
    sqlx::query("DROP TRIGGER proposal_content_immutable_update")
        .execute(f.db.pool())
        .await
        .expect("allow a malformed persisted Proposal fixture");
    sqlx::query("UPDATE proposal SET target_kind = 'corrupt-kind' WHERE id = ?")
        .bind(&proposal_b_corrupt.id)
        .execute(f.db.pool())
        .await
        .expect("corrupt Project B Proposal target kind");

    let mut decision_errors = Vec::new();
    for (proposal_id, proposal_version) in [
        ("pr4-missing-proposal", 1),
        (proposal_b_open.id.as_str(), proposal_b_open.content_version),
        (proposal_b_open.id.as_str(), 99),
        (
            proposal_b_resolved.id.as_str(),
            proposal_b_resolved.content_version,
        ),
        (
            proposal_b_corrupt.id.as_str(),
            proposal_b_corrupt.content_version,
        ),
    ] {
        decision_errors.push(
            f.service
                .record_decision(
                    decision_input(
                        &f.task_id,
                        proposal_id,
                        proposal_version,
                        DecisionOutcome::Approve,
                    ),
                    vec![human(&f)],
                )
                .await
                .expect_err("missing and foreign Proposal references are indistinguishable"),
        );
    }
    assert!(decision_errors.iter().all(|error| matches!(
        error,
        services::ServiceError::NotFound {
            entity: "proposal",
            ..
        }
    )));

    let stale_same_task = f
        .service
        .record_decision(
            decision_input(
                &f.task_id,
                &proposal_a.id,
                proposal_a.content_version + 1,
                DecisionOutcome::Approve,
            ),
            vec![human(&f)],
        )
        .await
        .expect_err("same-Task stale version remains semantic");
    assert!(matches!(
        stale_same_task,
        services::ServiceError::InvalidOperation { .. }
    ));
    f.service
        .record_decision(
            decision_input(
                &f.task_id,
                &proposal_a.id,
                proposal_a.content_version,
                DecisionOutcome::Approve,
            ),
            vec![human(&f)],
        )
        .await
        .expect("resolve same-Task Proposal");
    let resolved_same_task = f
        .service
        .record_decision(
            decision_input(
                &f.task_id,
                &proposal_a.id,
                proposal_a.content_version,
                DecisionOutcome::Approve,
            ),
            vec![human(&f)],
        )
        .await
        .expect_err("same-Task resolved Proposal remains semantic");
    assert!(matches!(
        resolved_same_task,
        services::ServiceError::InvalidOperation { .. }
    ));

    for supersedes_id in [
        "pr4-missing-supersedes-proposal",
        proposal_b_open.id.as_str(),
        proposal_b_resolved.id.as_str(),
        proposal_b_corrupt.id.as_str(),
    ] {
        let mut input = task_proposal(&f.task_id);
        input.supersedes_proposal_id = Some(supersedes_id.to_owned());
        let error = f
            .service
            .create_proposal(human(&f), input)
            .await
            .expect_err("missing and foreign supersedes references are not found");
        assert!(matches!(
            error,
            services::ServiceError::NotFound {
                entity: "proposal",
                ..
            }
        ));
    }
    let mut same_task_supersedes = task_proposal(&f.task_id);
    same_task_supersedes.supersedes_proposal_id = Some(proposal_a.id.clone());
    let same_task_not_superseded = f
        .service
        .create_proposal(human(&f), same_task_supersedes)
        .await
        .expect_err("same-Task Proposal without a supersede Decision is semantic");
    assert!(matches!(
        same_task_not_superseded,
        services::ServiceError::InvalidOperation { .. }
    ));

    let artifact_a = f
        .service
        .create_artifact(
            human(&f),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: f.execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some("Project A artifact".to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect("Project A Artifact");
    let artifact_b = f
        .service
        .create_artifact(
            CollaborationActorSource::Human(foreign.user_id.clone()),
            CreateArtifactInput {
                task_id: foreign.task_id.clone(),
                producer_execution_id: foreign.execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some("Project B artifact".to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect("Project B principal creates its Artifact");

    for artifact_id in ["pr4-missing-artifact", artifact_b.id.as_str()] {
        let mut input = task_proposal(&f.task_id);
        input.artifact_ids = vec![artifact_id.to_owned()];
        let error = f
            .service
            .create_proposal(human(&f), input)
            .await
            .expect_err("missing and healthy foreign Artifact references are not found");
        assert!(matches!(
            error,
            services::ServiceError::NotFound {
                entity: "artifact",
                ..
            }
        ));
    }

    sqlx::query("DROP TRIGGER artifact_execution_producer_immutable_delete")
        .execute(f.db.pool())
        .await
        .expect("allow corrupt Artifact producer fixtures");
    for artifact_id in [&artifact_a.id, &artifact_b.id] {
        sqlx::query("DELETE FROM artifact_execution_producer WHERE artifact_id = ?")
            .bind(artifact_id)
            .execute(f.db.pool())
            .await
            .expect("corrupt Artifact producer relation");
    }
    let foreign_corrupt_artifact = f
        .service
        .create_proposal(human(&f), {
            let mut input = task_proposal(&f.task_id);
            input.artifact_ids = vec![artifact_b.id.clone()];
            input
        })
        .await
        .expect_err("foreign Artifact corruption is hidden by Task scope");
    assert!(matches!(
        foreign_corrupt_artifact,
        services::ServiceError::NotFound {
            entity: "artifact",
            ..
        }
    ));
    let same_task_corrupt_artifact = f
        .service
        .create_proposal(human(&f), {
            let mut input = task_proposal(&f.task_id);
            input.artifact_ids = vec![artifact_a.id.clone()];
            input
        })
        .await
        .expect_err("same-Task Artifact producer corruption fails closed");
    assert!(matches!(
        same_task_corrupt_artifact,
        services::ServiceError::Db(db::DbError::Check(_))
    ));
}

#[tokio::test]
async fn foreign_execution_role_workspace_and_actor_references_are_scoped_first() {
    let f = fixture().await;
    sqlx::query("UPDATE project SET owner_id = ? WHERE id = ?")
        .bind(&f.user_id)
        .bind(&f.project_id)
        .execute(f.db.pool())
        .await
        .expect("Project A owner");
    let foreign = create_foreign_project(&f).await;
    let now = now_rfc3339();
    let role_a = TaskRoleRepo::create(
        &*f.db,
        CreateTaskRole {
            id: "pr4-role-a".to_owned(),
            task_id: f.task_id.clone(),
            role: "reviewer".to_owned(),
            coordination_mode: Some(CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project A Role");
    let role_b = TaskRoleRepo::create(
        &*f.db,
        CreateTaskRole {
            id: "pr4-role-b".to_owned(),
            task_id: foreign.task_id.clone(),
            role: "reviewer".to_owned(),
            coordination_mode: Some(CoordinationMode::Collaborative),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Project B Role");
    AgentRepo::create_identity_with_profile(
        &*f.db,
        db::CreateAgentIdentity {
            id: "pr4-agent-b".to_owned(),
            name: "Project B Agent".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(foreign.user_id.clone()),
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        db::CreateAgentProfile {
            id: "pr4-agent-profile-b".to_owned(),
            identity_id: "pr4-agent-b".to_owned(),
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
    .expect("Project B Agent without a Project A TaskRole membership");
    for (workspace_id, task_id, repo_id, path) in [
        (
            "pr4-workspace-a",
            f.task_id.as_str(),
            f.repo_id.as_str(),
            "/tmp/pr4-workspace-a",
        ),
        (
            "pr4-workspace-b",
            foreign.task_id.as_str(),
            foreign.repo_id.as_str(),
            "/tmp/pr4-workspace-b",
        ),
    ] {
        sqlx::query(
            "INSERT INTO workspace (
                 id, task_id, repo_id, worktree_path, branch, status,
                 before_sha, created_at, updated_at
             ) VALUES (?, ?, ?, ?, 'main', 'ready', NULL, ?, ?)",
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(repo_id)
        .bind(path)
        .bind(&now)
        .bind(&now)
        .execute(f.db.pool())
        .await
        .expect("workspace");
    }

    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(f.db.pool())
        .await
        .expect("permit malformed reference fixtures");
    sqlx::query("DROP TRIGGER task_role_coordination_mode_guard_update")
        .execute(f.db.pool())
        .await
        .expect("allow a malformed TaskRole fixture");
    sqlx::query("UPDATE execution SET status = 'corrupt-status' WHERE id = ?")
        .bind(&foreign.execution_id)
        .execute(f.db.pool())
        .await
        .expect("corrupt foreign Execution status");
    sqlx::query("UPDATE task_role SET coordination_mode = 'corrupt-mode' WHERE id = ?")
        .bind(&role_b.id)
        .execute(f.db.pool())
        .await
        .expect("corrupt foreign TaskRole mode");
    sqlx::query("UPDATE workspace SET status = 'corrupt-status' WHERE id = 'pr4-workspace-b'")
        .execute(f.db.pool())
        .await
        .expect("corrupt foreign Workspace status");
    sqlx::query("UPDATE agent_identity SET status = 'corrupt-status' WHERE id = 'pr4-agent-b'")
        .execute(f.db.pool())
        .await
        .expect("corrupt foreign Agent status");
    sqlx::query("UPDATE execution SET status = 'corrupt-status' WHERE id = ?")
        .bind(&f.execution_id)
        .execute(f.db.pool())
        .await
        .expect("corrupt in-Task Execution status");
    sqlx::query("UPDATE workspace SET status = 'corrupt-status' WHERE id = 'pr4-workspace-a'")
        .execute(f.db.pool())
        .await
        .expect("corrupt in-Task Workspace status");
    sqlx::query("PRAGMA ignore_check_constraints = OFF")
        .execute(f.db.pool())
        .await
        .expect("restore SQLite checks");

    let foreign_execution_as_producer = f
        .service
        .create_artifact(
            human(&f),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: foreign.execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some("invalid producer".to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect_err("foreign producer is rejected before Execution mapping");
    assert!(matches!(
        foreign_execution_as_producer,
        services::ServiceError::NotFound {
            entity: "execution",
            ..
        }
    ));

    let foreign_execution_as_parent = f
        .service
        .create_handoff(
            human(&f),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                work_unit_id: None,
                source_role_id: None,
                target: CollaborationTarget::Task,
                intent: db::HandoffIntent::Question,
                parent_execution_id: Some(foreign.execution_id.clone()),
                expected_policy_ref: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect_err("foreign parent Execution is rejected before mapping");
    assert!(matches!(
        foreign_execution_as_parent,
        services::ServiceError::NotFound {
            entity: "execution",
            ..
        }
    ));

    let foreign_execution_source = f
        .service
        .create_message(
            CollaborationActorSource::Execution(foreign.execution_id.clone()),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                work_unit_id: None,
                target: CollaborationTarget::Task,
                body: "foreign source".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect_err("foreign Execution source is scoped before mapping");
    assert!(matches!(
        foreign_execution_source,
        services::ServiceError::NotFound {
            entity: "execution",
            ..
        }
    ));

    for source_role_id in ["pr4-missing-source-role", role_b.id.as_str()] {
        let error = f
            .service
            .create_handoff(
                human(&f),
                CreateHandoffInput {
                    task_id: f.task_id.clone(),
                    work_unit_id: None,
                    source_role_id: Some(source_role_id.to_owned()),
                    target: CollaborationTarget::Task,
                    intent: db::HandoffIntent::Question,
                    parent_execution_id: None,
                    expected_policy_ref: None,
                    artifact_ids: vec![],
                },
            )
            .await
            .expect_err("unknown and foreign source Role use Task-scoped membership");
        assert!(matches!(
            error,
            services::ServiceError::AuthorizationDenied { .. }
        ));
    }

    for target in [
        ProposalTarget {
            kind: ProposalTargetKind::Execution,
            id: foreign.execution_id.clone(),
        },
        ProposalTarget {
            kind: ProposalTargetKind::Workspace,
            id: "pr4-workspace-b".to_owned(),
        },
    ] {
        let mut input = task_proposal(&f.task_id);
        input.target = target;
        let error = f
            .service
            .create_proposal(human(&f), input)
            .await
            .expect_err("foreign target is rejected before semantic mapping");
        assert!(matches!(error, services::ServiceError::NotFound { .. }));
    }

    for target in [
        CollaborationTarget::Role(role_b.id.clone()),
        CollaborationTarget::Actor(db::ActorRef::Human(foreign.user_id.clone())),
        CollaborationTarget::Actor(db::ActorRef::Agent("pr4-agent-b".to_owned())),
    ] {
        let message_error = f
            .service
            .create_message(
                human(&f),
                CreateMessageInput {
                    task_id: f.task_id.clone(),
                    work_unit_id: None,
                    target: target.clone(),
                    body: "foreign target".to_owned(),
                    artifact_ids: vec![],
                },
            )
            .await
            .expect_err("foreign Message target is rejected before semantic mapping");
        assert!(matches!(
            message_error,
            services::ServiceError::NotFound { .. }
        ));
        let handoff_error = f
            .service
            .create_handoff(
                human(&f),
                CreateHandoffInput {
                    task_id: f.task_id.clone(),
                    work_unit_id: None,
                    source_role_id: None,
                    target,
                    intent: db::HandoffIntent::Question,
                    parent_execution_id: None,
                    expected_policy_ref: None,
                    artifact_ids: vec![],
                },
            )
            .await
            .expect_err("foreign Handoff target is rejected before semantic mapping");
        assert!(matches!(
            handoff_error,
            services::ServiceError::NotFound { .. }
        ));
    }

    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(f.db.pool())
        .await
        .expect("permit malformed in-Task TaskRole fixture");
    sqlx::query("UPDATE task_role SET coordination_mode = 'corrupt-mode' WHERE id = ?")
        .bind(&role_a.id)
        .execute(f.db.pool())
        .await
        .expect("corrupt in-Task TaskRole mode");
    sqlx::query("PRAGMA ignore_check_constraints = OFF")
        .execute(f.db.pool())
        .await
        .expect("restore SQLite checks");

    for (name, error) in [
        (
            "Execution",
            f.service
                .create_artifact(
                    human(&f),
                    CreateArtifactInput {
                        task_id: f.task_id.clone(),
                        producer_execution_id: f.execution_id.clone(),
                        kind: ArtifactKind::Summary,
                        storage_kind: ArtifactStorageKind::Inline,
                        content: Some("in-Task producer".to_owned()),
                        content_ref: None,
                        metadata_json: "{}".to_owned(),
                        digest: None,
                    },
                )
                .await
                .expect_err("in-Task Execution corruption fails closed"),
        ),
        (
            "TaskRole",
            f.service
                .create_message(
                    human(&f),
                    CreateMessageInput {
                        task_id: f.task_id.clone(),
                        work_unit_id: None,
                        target: CollaborationTarget::Role(role_a.id.clone()),
                        body: "in-Task role".to_owned(),
                        artifact_ids: vec![],
                    },
                )
                .await
                .expect_err("in-Task TaskRole corruption fails closed"),
        ),
        (
            "Workspace",
            f.service
                .create_proposal(human(&f), {
                    let mut input = task_proposal(&f.task_id);
                    input.target = ProposalTarget {
                        kind: ProposalTargetKind::Workspace,
                        id: "pr4-workspace-a".to_owned(),
                    };
                    input
                })
                .await
                .expect_err("in-Task Workspace corruption fails closed"),
        ),
    ] {
        assert!(
            matches!(
                &error,
                services::ServiceError::Db(db::DbError::InvalidTransition)
            ),
            "same-Task corrupt {name} should fail closed: {error:?}"
        );
    }
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
                work_unit_id: None,
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
                work_unit_id: None,
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
                work_unit_id: None,
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
                work_unit_id: None,
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

#[tokio::test]
async fn historical_collaboration_reads_survive_actor_and_workspace_deletion() {
    let f = fixture().await;
    let reader_id = "pr4-history-reader";
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES (?, 'pr4-reader@example.test', 'test', ?, ?)",
    )
    .bind(reader_id)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("authorized reader user");

    let artifact = f
        .service
        .create_artifact(
            human(&f),
            CreateArtifactInput {
                task_id: f.task_id.clone(),
                producer_execution_id: f.execution_id.clone(),
                kind: ArtifactKind::Summary,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some("historical content".to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: None,
            },
        )
        .await
        .expect("Artifact");
    let message = f
        .service
        .create_message(
            human(&f),
            CreateMessageInput {
                task_id: f.task_id.clone(),
                work_unit_id: None,
                target: CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone())),
                body: "historical message".to_owned(),
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Message");
    let handoff = f
        .service
        .create_handoff(
            human(&f),
            CreateHandoffInput {
                task_id: f.task_id.clone(),
                work_unit_id: None,
                source_role_id: None,
                target: CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone())),
                intent: db::HandoffIntent::Question,
                parent_execution_id: Some(f.execution_id.clone()),
                expected_policy_ref: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Handoff");
    let workspace = WorkspaceRepo::create(
        &*f.db,
        CreateWorkspace {
            id: "pr4-history-workspace".to_owned(),
            task_id: f.task_id.clone(),
            repo_id: "pr4-repo".to_owned(),
            worktree_path: "/tmp/pr4-history-workspace".to_owned(),
            branch: "pr4-history".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("Workspace");
    let workspace_proposal = f
        .service
        .create_proposal(
            human(&f),
            CreateProposalInput {
                task_id: f.task_id.clone(),
                target: ProposalTarget {
                    kind: ProposalTargetKind::Workspace,
                    id: workspace.id.clone(),
                },
                action: "inspect-workspace".to_owned(),
                reason: "historical workspace reference".to_owned(),
                target_version: None,
                target_digest: None,
                required_policy_ref: None,
                required_policy_version: None,
                required_policy_digest: None,
                supersedes_proposal_id: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Workspace Proposal");
    let decided_proposal = f
        .service
        .create_proposal(
            human(&f),
            CreateProposalInput {
                task_id: f.task_id.clone(),
                target: ProposalTarget {
                    kind: ProposalTargetKind::Task,
                    id: f.task_id.clone(),
                },
                action: "record-only".to_owned(),
                reason: "Decision history fixture".to_owned(),
                target_version: None,
                target_digest: None,
                required_policy_ref: None,
                required_policy_version: None,
                required_policy_digest: None,
                supersedes_proposal_id: None,
                artifact_ids: vec![],
            },
        )
        .await
        .expect("Task Proposal");
    let decision = f
        .service
        .record_decision(
            CreateDecisionInput {
                task_id: f.task_id.clone(),
                proposal_id: decided_proposal.id,
                proposal_version: 1,
                outcome: DecisionOutcome::Approve,
                rationale: "recorded only".to_owned(),
                policy_ref: None,
                policy_version: None,
                policy_digest: None,
            },
            vec![human(&f)],
        )
        .await
        .expect("Decision");

    assert!(UserRepo::delete_user(&*f.db, &f.user_id)
        .await
        .expect("Human deletion succeeds"));
    WorkspaceRepo::delete(&*f.db, &workspace.id)
        .await
        .expect("Workspace reset deletes live target");
    let reader = CollaborationActorSource::Human(reader_id.to_owned());

    assert_eq!(
        f.service
            .get_artifact(&artifact.id, reader.clone())
            .await
            .expect("Artifact remains readable")
            .producer,
        db::ActorRef::Human(f.user_id.clone())
    );
    let message = f
        .service
        .get_message(&message.id, reader.clone())
        .await
        .expect("Message remains readable");
    assert_eq!(message.sender, db::ActorRef::Human(f.user_id.clone()));
    assert_eq!(
        message.target,
        CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone()))
    );
    let handoff = f
        .service
        .get_handoff(&handoff.id, reader.clone())
        .await
        .expect("Handoff remains readable");
    assert_eq!(handoff.created_by, db::ActorRef::Human(f.user_id.clone()));
    assert_eq!(
        handoff.target,
        CollaborationTarget::Actor(db::ActorRef::Human(f.user_id.clone()))
    );
    let proposal = f
        .service
        .get_proposal(&workspace_proposal.id, reader.clone())
        .await
        .expect("Proposal remains readable after Workspace reset");
    assert_eq!(proposal.proposer, db::ActorRef::Human(f.user_id.clone()));
    assert_eq!(proposal.target.id, workspace.id);
    let proposal_page = f
        .service
        .list_proposals(
            &f.task_id,
            reader.clone(),
            db::PageRequest {
                cursor: None,
                limit: 10,
                include_total: true,
                sort_by: db::SortBy::CreatedAt,
                sort_order: db::SortOrder::Desc,
            },
        )
        .await
        .expect("proposal list survives a deleted target");
    assert_eq!(proposal_page.total_count, Some(2));
    assert!(proposal_page
        .items
        .iter()
        .any(|item| item.id == workspace_proposal.id));
    assert_eq!(
        f.service
            .get_decision(&decision.id, reader)
            .await
            .expect("Decision remains readable")
            .actors,
        vec![db::ActorRef::Human(f.user_id.clone())]
    );
}

#[tokio::test]
async fn proposal_target_insert_guard_rejects_future_and_unknown_kinds() {
    let f = fixture().await;
    let now = now_rfc3339();
    let workspace = WorkspaceRepo::create(
        &*f.db,
        CreateWorkspace {
            id: "pr4-target-workspace".to_owned(),
            task_id: f.task_id.clone(),
            repo_id: "pr4-repo".to_owned(),
            worktree_path: "/tmp/pr4-target-workspace".to_owned(),
            branch: "pr4-target".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("Workspace");
    let schema: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'proposal'",
    )
    .fetch_one(f.db.pool())
    .await
    .expect("Proposal schema");
    assert!(!schema.contains("target_kind IN"));

    for (id, kind, target_id) in [
        ("pr4-target-task", "task", f.task_id.as_str()),
        ("pr4-target-execution", "execution", f.execution_id.as_str()),
        (
            "pr4-target-workspace-valid",
            "workspace",
            workspace.id.as_str(),
        ),
    ] {
        sqlx::query(
            "INSERT INTO proposal (
                 id, task_id, proposer_actor_kind, proposer_actor_id,
                 target_kind, target_id, action, reason, status, created_at
             ) VALUES (?, ?, 'human', ?, ?, ?, 'inspect', 'valid target', 'open', ?)",
        )
        .bind(id)
        .bind(&f.task_id)
        .bind(&f.user_id)
        .bind(kind)
        .bind(target_id)
        .bind(&now)
        .execute(f.db.pool())
        .await
        .expect("PR4 target kind admitted");
    }
    for (id, kind) in [
        ("pr4-target-work-unit", "work_unit"),
        ("pr4-target-unknown", "whatever_unknown"),
    ] {
        let rejected = sqlx::query(
            "INSERT INTO proposal (
                 id, task_id, proposer_actor_kind, proposer_actor_id,
                 target_kind, target_id, action, reason, status, created_at
             ) VALUES (?, ?, 'human', ?, ?, 'opaque', 'inspect', 'invalid target', 'open', ?)",
        )
        .bind(id)
        .bind(&f.task_id)
        .bind(&f.user_id)
        .bind(kind)
        .bind(&now)
        .execute(f.db.pool())
        .await
        .expect_err("PR4 insert trigger remains closed");
        assert!(rejected.to_string().contains("Proposal ActorRef, target"));
    }
    assert!(
        serde_json::from_value::<api_types::ProposalTargetKind>(serde_json::json!(
            "validation_run"
        ))
        .is_err()
    );
}

#[tokio::test]
async fn collaboration_message_cursor_pages_stably_through_timestamp_ties() {
    let f = fixture().await;
    let rows = [
        ("tie-a", "2026-09-03T00:00:00Z"),
        ("tie-b", "2026-09-03T00:00:00Z"),
        ("middle", "2026-09-02T00:00:00Z"),
        ("old-a", "2026-09-01T00:00:00Z"),
        ("old-b", "2026-09-01T00:00:00Z"),
    ];
    for (id, created_at) in rows {
        sqlx::query(
            "INSERT INTO message (
                 id, task_id, sender_actor_kind, sender_actor_id,
                 target_kind, body, created_at
             ) VALUES (?, ?, 'human', ?, 'task', 'page row', ?)",
        )
        .bind(id)
        .bind(&f.task_id)
        .bind(&f.user_id)
        .bind(created_at)
        .execute(f.db.pool())
        .await
        .expect("message row");
    }
    let page_request = |cursor| db::PageRequest {
        cursor,
        limit: 2,
        include_total: true,
        sort_by: db::SortBy::CreatedAt,
        sort_order: db::SortOrder::Desc,
    };
    let first = CollaborationRepo::list_messages(&*f.db, &f.task_id, page_request(None))
        .await
        .expect("first page");
    assert_eq!(first.total_count, Some(5));
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["tie-b", "tie-a"]
    );
    let cursor_1 = first.next_cursor.clone().expect("first next cursor");
    let second = CollaborationRepo::list_messages(&*f.db, &f.task_id, page_request(Some(cursor_1)))
        .await
        .expect("second page");
    assert_eq!(second.total_count, Some(5));
    assert_eq!(
        second
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["middle", "old-b"]
    );
    let cursor_2 = second.next_cursor.clone().expect("second next cursor");
    let third = CollaborationRepo::list_messages(&*f.db, &f.task_id, page_request(Some(cursor_2)))
        .await
        .expect("third page");
    assert_eq!(third.total_count, Some(5));
    assert_eq!(
        third
            .items
            .iter()
            .map(|item| item.id.as_str())
            .collect::<Vec<_>>(),
        vec!["old-a"]
    );
    assert!(third.next_cursor.is_none());

    let all_ids: Vec<&str> = first
        .items
        .iter()
        .chain(second.items.iter())
        .chain(third.items.iter())
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(all_ids, vec!["tie-b", "tie-a", "middle", "old-b", "old-a"]);
    let invalid_cursor = CollaborationRepo::list_messages(
        &*f.db,
        &f.task_id,
        page_request(Some("invalid-cursor".to_owned())),
    )
    .await
    .expect_err("invalid cursor is rejected");
    assert!(matches!(invalid_cursor, db::DbError::InvalidCursor));
}

#[tokio::test]
async fn legacy_production_writers_do_not_project_into_generic_collaboration() {
    let f = fixture().await;
    let plan_root = tempfile::tempdir().expect("plan root");
    let worktree = plan_root.path().join("repo");
    std::fs::create_dir(&worktree).expect("legacy worktree");
    std::fs::write(
        plan_root.path().join("plan.md"),
        "# Legacy plan\n- [ ] keep legacy\n",
    )
    .expect("legacy plan file");
    services::plan_artifact::capture_plan_revision(
        &f.db,
        &f.task_id,
        &worktree,
        "approved",
        Some(&f.execution_id),
    )
    .await
    .expect("legacy planning writer");

    let now = now_rfc3339();
    let main_chat = AgentChatRepo::get_main_chat(&*f.db, &f.user_id)
        .await
        .expect("legacy Main Agent Chat lookup")
        .expect("legacy Main Agent Chat exists");
    let project_chat = AgentChatRepo::get_project_chat(&*f.db, &f.project_id)
        .await
        .expect("legacy Project Agent Chat lookup")
        .expect("legacy Project Agent Chat exists");
    AgentChatMessageRepo::append_agent_chat_message(
        &*f.db,
        CreateAgentChatMessage {
            id: "pr4-legacy-chat-message".to_owned(),
            chat_id: main_chat.id.clone(),
            sequence: 1,
            author_type: AgentChatMessageAuthorType::User,
            author_id: Some(f.user_id.clone()),
            content: "legacy Agent Chat message".to_owned(),
            content_guard_json: "{}".to_owned(),
            sensitivity: "internal".to_owned(),
            status: AgentChatMessageStatus::Complete,
            outcome: None,
            model: None,
            profile_id: None,
            session_id: None,
            context_manifest_id: None,
            token_usage_json: None,
            duration_ms: None,
            error: None,
            correlation_id: "pr4-legacy-chat-correlation".to_owned(),
            causation_id: None,
            handoff_id: None,
            source_type: "native".to_owned(),
            source_id: None,
            source_message_id: None,
            source_room_id: None,
            source_conversation_id: None,
            source_sequence: None,
            source_metadata_json: "{}".to_owned(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("legacy Agent Chat message writer");
    AgentHandoffRepo::create_agent_handoff(
        &*f.db,
        CreateAgentHandoff {
            id: "pr4-legacy-agent-handoff".to_owned(),
            source_chat_id: main_chat.id,
            target_chat_id: project_chat.id,
            source_message_id: None,
            source_turn_job_id: None,
            author_identity_id: None,
            content: "legacy Agent Handoff".to_owned(),
            content_guard_json: "{}".to_owned(),
            source_revisions_json: "[]".to_owned(),
            correlation_id: "pr4-legacy-handoff-correlation".to_owned(),
            causation_id: None,
            dedupe_key: "pr4-legacy-handoff-dedupe".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("legacy Agent Handoff writer");

    let project = ProjectRepo::get_by_id(&*f.db, &f.project_id)
        .await
        .expect("Project lookup")
        .expect("Project exists");
    ProjectOrchestrationRepo::append_project_decision(
        &*f.db,
        CreateProjectDecision {
            id: "pr4-legacy-project-decision".to_owned(),
            project_id: f.project_id.clone(),
            expected_project_version: project.version,
            state: "active".to_owned(),
            decision_class: "project_implementation".to_owned(),
            question: "legacy Project Decision".to_owned(),
            context_json: "{}".to_owned(),
            options_json: "[]".to_owned(),
            selected_outcome: "keep-legacy".to_owned(),
            rationale: "legacy decision writer".to_owned(),
            principal_type: "human".to_owned(),
            principal_id: f.user_id.clone(),
            authority_basis: "explicit-user-choice".to_owned(),
            authorization_action: "project.decision.record".to_owned(),
            explicit_event: "legacy-explicit-event".to_owned(),
            authorization_occurred_at: now.clone(),
            charter_revision_id: None,
            baseline_revision_id: None,
            source_refs_json: "[]".to_owned(),
            affected_records_json: "{}".to_owned(),
            supersedes_decision_id: None,
            created_at: now,
        },
    )
    .await
    .expect("legacy Project Decision writer");

    let legacy_counts = [
        ("task_plan_revision", "task_plan_revision"),
        ("agent_chat_message", "agent_chat_message"),
        ("agent_handoff", "agent_handoff"),
        ("project_decision", "project_decision"),
    ];
    for (table, _) in legacy_counts {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(f.db.pool())
            .await
            .expect("legacy rows remain written");
        assert_eq!(count, 1, "legacy production writer populated {table}");
    }
    for table in ["artifact", "message", "handoff", "proposal", "decision"] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(f.db.pool())
            .await
            .expect("generic table exists");
        assert_eq!(count, 0, "legacy writer did not project to generic {table}");
    }
}
