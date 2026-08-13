use std::sync::Arc;

use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, AgentRepo, AgentStatus,
    AttentionRepo, CreateAgentIdentity, CreateAgentProfile, CreateDomainEvent, DomainEventRepo,
    SqliteDb,
};
use services::{
    AttentionService, WakeAdmissionRequest, WakeAdmissionResult, WakeSuppressionReason,
};
use tokio::sync::watch;

async fn database() -> Arc<SqliteDb> {
    let pool = create_sqlite_pool("sqlite::memory:").await.unwrap();
    run_migrations(&pool).await.unwrap();
    Arc::new(SqliteDb::new(pool))
}

async fn identity(db: &SqliteDb, id: &str) {
    let now = now_rfc3339();
    AgentRepo::create_identity_with_profile(
        db,
        CreateAgentIdentity {
            id: id.to_owned(),
            name: "wake-test".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "account".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        CreateAgentProfile {
            id: new_uuid_v4(),
            identity_id: id.to_owned(),
            backend_kind: "native".to_owned(),
            executor_type: "embedded".to_owned(),
            provider: Some("test".to_owned()),
            model: Some("test".to_owned()),
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "[]".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .unwrap();
}

fn request(identity_id: &str, incident_key: &str) -> WakeAdmissionRequest {
    WakeAdmissionRequest {
        identity_id: identity_id.to_owned(),
        scope_type: "account".to_owned(),
        scope_id: "account-1".to_owned(),
        incident_key: incident_key.to_owned(),
        lease_owner: "worker-1".to_owned(),
        correlation_id: "correlation-1".to_owned(),
        causation_id: Some("event-1".to_owned()),
        caused_by_identity_id: None,
        reaction_depth: 0,
        now: "2026-01-01T00:00:00Z".to_owned(),
        lease_seconds: 30,
        cooldown_seconds: 60,
    }
}

#[tokio::test]
async fn wake_admission_deduplicates_and_suppresses_recursive_events() {
    let db = database().await;
    let identity_id = new_uuid_v4();
    identity(&db, &identity_id).await;
    let service = AttentionService::new(Arc::clone(&db));

    let first = service
        .admit_wake(request(&identity_id, "incident-1"))
        .await
        .unwrap();
    assert!(matches!(first, WakeAdmissionResult::Admitted { .. }));

    let duplicate = service
        .admit_wake(request(&identity_id, "incident-1"))
        .await
        .unwrap();
    assert!(matches!(
        duplicate,
        WakeAdmissionResult::Suppressed {
            reason: WakeSuppressionReason::DuplicateIncident
        }
    ));

    let mut recursive = request(&identity_id, "incident-2");
    recursive.reaction_depth = 9;
    assert!(matches!(
        service.admit_wake(recursive).await.unwrap(),
        WakeAdmissionResult::Suppressed {
            reason: WakeSuppressionReason::ReactionDepthExceeded
        }
    ));

    let mut self_event = request(&identity_id, "incident-3");
    self_event.reaction_depth = 1;
    self_event.caused_by_identity_id = Some(identity_id.clone());
    assert!(matches!(
        service.admit_wake(self_event).await.unwrap(),
        WakeAdmissionResult::Suppressed {
            reason: WakeSuppressionReason::SelfEvent
        }
    ));
}

#[tokio::test]
async fn projection_worker_drains_events_reports_health_and_stops() {
    let db = database().await;
    let project_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO project (
            id, name, settings, workflow_definition, owner_id, created_at, updated_at
         ) VALUES (?, 'attention-worker-project', '{}', '{}', NULL, ?, ?)",
    )
    .bind(&project_id)
    .bind(now_rfc3339())
    .bind(now_rfc3339())
    .execute(db.pool())
    .await
    .unwrap();
    DomainEventRepo::append_event(
        &*db,
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.validation_failed".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: new_uuid_v4(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "project".to_owned(),
            scope_id: project_id,
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(new_uuid_v4()),
            payload_json: "{}".to_owned(),
            created_at: now_rfc3339(),
        },
    )
    .await
    .unwrap();

    let service = std::sync::Arc::new(AttentionService::new(std::sync::Arc::clone(&db)));
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let handle = std::sync::Arc::clone(&service).start(shutdown_rx);
    let mut health = None;
    for _ in 0..100 {
        health = AttentionRepo::get_attention_consumer_health(&*db, "attention_projection")
            .await
            .unwrap();
        if health
            .as_ref()
            .is_some_and(|value| value.processed_events >= 1)
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(health.is_some_and(|value| value.processed_events >= 1));

    shutdown_tx.send(true).unwrap();
    handle.await.unwrap();
}

#[tokio::test]
async fn projected_incident_emits_one_durable_wake_action_and_replay_is_suppressed() {
    let db = database().await;
    let identity_id = new_uuid_v4();
    identity(&db, &identity_id).await;
    let source_event_id = new_uuid_v4();
    DomainEventRepo::append_event(
        &*db,
        CreateDomainEvent {
            id: source_event_id.clone(),
            event_type: "runtime.connection_unavailable".to_owned(),
            entity_type: "agent_session".to_owned(),
            entity_id: new_uuid_v4(),
            actor_type: "agent".to_owned(),
            actor_id: Some(identity_id.clone()),
            scope_type: "account".to_owned(),
            scope_id: "account-1".to_owned(),
            correlation_id: "wake-correlation".to_owned(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some("wake-source-event".to_owned()),
            payload_json: r#"{"status":"unavailable"}"#.to_owned(),
            created_at: now_rfc3339(),
        },
    )
    .await
    .unwrap();

    let service = AttentionService::new(Arc::clone(&db));
    let run = service.project_once(100).await.unwrap();
    assert_eq!(run.processed_events, 1);

    let wake_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'agent.wake.admitted'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(wake_count, 1);

    let incident_key = format!(
        "attention:runtime_offline:account:account-1:agent_session:{}",
        sqlx::query_scalar::<_, String>("SELECT entity_id FROM domain_event WHERE id = ?",)
            .bind(&source_event_id)
            .fetch_one(db.pool())
            .await
            .unwrap()
    );
    let replay = service
        .admit_wake(WakeAdmissionRequest {
            identity_id,
            scope_type: "account".to_owned(),
            scope_id: "account-1".to_owned(),
            incident_key,
            lease_owner: "replay-worker".to_owned(),
            correlation_id: "wake-correlation".to_owned(),
            causation_id: Some(source_event_id),
            caused_by_identity_id: None,
            reaction_depth: 0,
            now: now_rfc3339(),
            lease_seconds: 60,
            cooldown_seconds: 300,
        })
        .await
        .unwrap();
    assert!(matches!(
        replay,
        WakeAdmissionResult::Suppressed {
            reason: WakeSuppressionReason::DuplicateIncident
        }
    ));
    let wake_count_after_replay: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE event_type = 'agent.wake.admitted'",
    )
    .fetch_one(db.pool())
    .await
    .unwrap();
    assert_eq!(wake_count_after_replay, 1);
}
