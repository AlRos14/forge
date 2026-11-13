use super::tests::completed_pr6_execution_fixture;
use super::*;
use crate::collaboration_service::CollaborationActorSource;
use db::{
    CreateProjectMember, CreateRoleMembership, CreateTaskRole, ProjectMemberRepo,
    RoleMembershipRepo, TaskRoleRepo, WorkUnitRepo,
};
use sqlx::Row;

fn review_findings_output(
    request: &db::TaskCoordinationRequest,
    decision: &crate::task_rework::ReviewFindingsDecision,
) -> String {
    json!({
        "schema_version": 1,
        "request_id": request.id,
        "task_id": request.task_id,
        "authorization_version": request.authorization_version,
        "review_findings": [decision],
    })
    .to_string()
}

async fn create_review_request(
    coordination: &crate::task_coordination::TaskCoordinationService,
    task_id: &str,
    report_id: &str,
) -> db::TaskCoordinationRequest {
    let (request, created) = coordination
        .request_review_findings_for_report(task_id, report_id)
        .await
        .unwrap()
        .expect("current ReviewReport creates an exact typed request");
    assert!(created);
    request
}

async fn start_review_request(
    coordination: &crate::task_coordination::TaskCoordinationService,
    database: &db::SqliteDb,
    request: &db::TaskCoordinationRequest,
    wake_id: &str,
    execution_id: &str,
) -> db::TaskCoordinationRequest {
    coordination
        .mark_turn_started(request, wake_id, execution_id)
        .await
        .unwrap();
    db::TaskCoordinationRepo::get_task_coordination_request(database, &request.id)
        .await
        .unwrap()
        .unwrap()
}

async fn submit_review_findings(
    coordination: &crate::task_coordination::TaskCoordinationService,
    request: &db::TaskCoordinationRequest,
    execution: &db::Execution,
    decision: &crate::task_rework::ReviewFindingsDecision,
) -> crate::Result<()> {
    let mut completed = execution.clone();
    completed.summary = Some(review_findings_output(request, decision));
    coordination.complete_turn(request, &completed).await
}

async fn triage_fixture(
    disposition: api_types::TaskReviewDisposition,
    extra_rechecks: usize,
    pause_before_provider_start: bool,
) {
    let mut fixture = completed_pr6_execution_fixture(Vec::new()).await;
    let pre_start_barriers = if pause_before_provider_start {
        let reached = Arc::new(tokio::sync::Barrier::new(2));
        let release = Arc::new(tokio::sync::Barrier::new(2));
        fixture.runtime.pre_start_test_barrier = Some((Arc::clone(&reached), Arc::clone(&release)));
        Some((reached, release))
    } else {
        None
    };
    let db = &fixture.database;
    let task_service = &fixture.runtime.task_service;
    let units = task_service.orchestrator_work_unit_service();
    let task = TaskRepo::get_by_id(&**db, &fixture.task_id, false)
        .await
        .unwrap()
        .unwrap();
    let project = db::ProjectRepo::get_by_id(&**db, &task.project_id)
        .await
        .unwrap()
        .unwrap();
    let owner = project.owner_id.unwrap();
    let reviewer = new_uuid_v4();
    let now = now_rfc3339();
    db::UserRepo::create_user(
        &**db,
        &db::User {
            id: reviewer.clone(),
            email: format!("{reviewer}@test.invalid"),
            password_hash: "fixture".into(),
            display_name: None,
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .unwrap();
    ProjectMemberRepo::add_member(
        &**db,
        CreateProjectMember {
            id: new_uuid_v4(),
            project_id: task.project_id.clone(),
            user_id: reviewer.clone(),
            role: "member".into(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .unwrap();
    let implementation_role = new_uuid_v4();
    let review_role = new_uuid_v4();
    for (id, name, actor, configuration) in [
        (
            &implementation_role,
            "implementer",
            &owner,
            json!({"schema_version":1,"capabilities":[],"rework":null}),
        ),
        (
            &review_role,
            "reviewer",
            &reviewer,
            json!({"schema_version":1,"capabilities":["formal_review","request_rework"],"rework":{"mode":"automatic","allowed_destination_role_ids":[implementation_role],"next_review_role_id":review_role,"correction_actor":{"kind":"original_contributor"},"review_actor":{"kind":"human","id":reviewer}}}),
        ),
    ] {
        TaskRoleRepo::create_with_configuration(
            &**db,
            CreateTaskRole {
                id: id.clone(),
                task_id: task.id.clone(),
                role: name.into(),
                coordination_mode: Some(db::CoordinationMode::Independent),
                policy_json: "{}".into(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            &configuration.to_string(),
        )
        .await
        .unwrap();
        RoleMembershipRepo::add(
            &**db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: id.clone(),
                actor_kind: ActorKind::Human,
                actor_id: actor.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .unwrap();
    }
    let create = |role: &str, actor: &str, subject: Option<String>| crate::CreateWorkUnitInput {
        task_id: task.id.clone(),
        title: format!("Exact {role}"),
        scope: "Inspect the exact acceptance evidence".into(),
        execution_instructions: None,
        role: role.into(),
        parent_work_unit_id: None,
        assigned_actor: Some(ActorRef::Human(actor.into())),
        requires_integration: false,
        provenance: None,
        review_subject: subject,
        artifact_inputs: Vec::new(),
        review_approval: None,
    };
    let source = CollaborationActorSource::Human(owner.clone());
    let implementation = units
        .create(source.clone(), create("implementer", &owner, None))
        .await
        .unwrap();
    let review = units
        .create(
            source.clone(),
            create("reviewer", &reviewer, Some(implementation.id.clone())),
        )
        .await
        .unwrap();
    let delivery_id = new_uuid_v4();
    sqlx::query("INSERT INTO execution(id,task_id,agent_id,role,status,actor_kind,actor_id,work_unit_id,work_unit_version,after_sha,created_at,updated_at) VALUES(?,?,NULL,'implementer','running','human',?,?,?, ?,?,?)")
        .bind(&delivery_id).bind(&task.id).bind(&owner).bind(&implementation.id).bind(implementation.version).bind("a".repeat(40)).bind(&now).bind(&now).execute(db.pool()).await.unwrap();
    sqlx::query("UPDATE execution SET status='completed' WHERE id=?")
        .bind(&delivery_id)
        .execute(db.pool())
        .await
        .unwrap();
    units
        .transition(
            source,
            &implementation.id,
            implementation.version,
            db::WorkUnitStatus::Completed,
        )
        .await
        .unwrap();
    let (_,mut report) = units.submit_human_review(task_service,&review.id,&reviewer,review.version,serde_json::from_value(json!({"verdict":"request_changes","summary":"Missing acceptance evidence","criteria":["acceptance evidence"],"findings":["The output has no acceptance evidence"]})).unwrap()).await.unwrap();
    let rework = crate::task_rework::TaskReworkService::new(db.clone(), fixture.event_bus.clone());
    let event_id: String = sqlx::query_scalar(
        "SELECT id FROM domain_event WHERE event_type='artifact.created' AND entity_id=?",
    )
    .bind(&report.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let event = DomainEventRepo::get_event(&**db, &event_id)
        .await
        .unwrap()
        .unwrap();
    rework.process_domain_event(&event).await.unwrap();
    let review_execution = ExecutionRepo::get_by_id(&**db, report.execution_producer().unwrap().0)
        .await
        .unwrap()
        .unwrap();
    assert!(
        task_service
            .review_read_paths_for_execution(&review_execution)
            .await
            .unwrap()
            .is_empty(),
        "artifact-only review has no implicit repository read grant"
    );

    let coordination = crate::TaskCoordinationService::new(db.clone(), fixture.event_bus.clone());
    let overview = coordination.overview(&task.id, &owner).await.unwrap();
    let authorized = coordination
        .authorize(
            &task.id,
            &owner,
            crate::AuthorizationInput {
                expected_task_version: overview.task.version,
                expected_coordination_version: overview.coordination.authorization_version,
                mode: db::TaskCoordinationMode::PrepareAndExecute,
                coordinator_task_role_id: Some(fixture.task_role_id.clone()),
                coordinator_actor: fixture.execution.actor_ref(),
                envelope: None,
                idempotency_key: "triage-authority".into(),
            },
        )
        .await
        .unwrap();
    let lifecycle_service =
        crate::task_lifecycle::TaskLifecycleService::new(db.clone(), fixture.event_bus.clone());
    let current_task = TaskRepo::get_by_id(&**db, &task.id, false)
        .await
        .unwrap()
        .unwrap();
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&**db, &task.id)
        .await
        .unwrap()
        .unwrap();
    lifecycle_service
        .launch_task(
            &task.id,
            current_task.version,
            lifecycle.version,
            &api_types::ActorRef::Human(owner.clone()),
            "review-triage-fixture-launch",
        )
        .await
        .unwrap();
    // The fixture explicitly budgets enough typed turns to retain coverage of
    // ten exact review reports without changing product defaults.
    sqlx::query("UPDATE task_coordination SET max_turns=32 WHERE task_id=?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .unwrap();
    let mut request = create_review_request(&coordination, &task.id, &report.id).await;
    assert!(fixture
        .runtime
        .classify_event(&event)
        .await
        .unwrap()
        .is_none());
    let exact_context = rework
        .exact_review_findings_context(&request)
        .await
        .unwrap();
    assert_eq!(
        exact_context["reviewed_result"]["execution_id"],
        delivery_id
    );
    assert_eq!(
        exact_context["reviewed_result"]["original_scope"],
        implementation.scope
    );
    assert_eq!(exact_context["review_work_unit_id"], review.id);
    assert_eq!(exact_context["review_scope"], review.scope);
    let request_event = sqlx::query(
        "SELECT id,sequence FROM domain_event
         WHERE event_type='task.coordination_requested'
           AND entity_type='task_coordination_request' AND entity_id=?",
    )
    .bind(&request.id)
    .fetch_one(db.pool())
    .await
    .unwrap();
    let request_event_id: String = request_event.try_get("id").unwrap();
    let request_event_sequence: i64 = request_event.try_get("sequence").unwrap();
    let request_domain_event = DomainEventRepo::get_event(&**db, &request_event_id)
        .await
        .unwrap()
        .unwrap();
    if pause_before_provider_start {
        let (reached, release) = pre_start_barriers
            .as_ref()
            .expect("pre-Start barrier is configured for this regression");
        assert!(
            fixture
                .runtime
                .reconcile_orchestrator_terminal(&fixture.terminal_event)
                .await
                .unwrap(),
            "fixture's completed same-Agent wake is terminal before admitting a new one"
        );
        assert_eq!(
            db::OrchestratorWakeRepo::get_orchestrator_wake(&**db, &fixture.wake.id)
                .await
                .unwrap()
                .unwrap()
                .state,
            db::OrchestratorWakeState::Completed
        );
        let signal = fixture
            .runtime
            .classify_event(&request_domain_event)
            .await
            .unwrap()
            .expect("exact typed request classifies for its coordinator");
        assert_eq!(
            fixture
                .runtime
                .admit_signal(&request_domain_event, signal)
                .await
                .unwrap(),
            1,
            "exact typed request admits one coordinator wake"
        );
        let wake_id: String =
            sqlx::query_scalar("SELECT id FROM orchestrator_wake WHERE event_id=? AND task_id=?")
                .bind(&request_event_id)
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let turn_count_before: i64 =
            sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        let dispatch_runtime = Arc::new(fixture.runtime.clone());
        let dispatch = tokio::spawn(async move { dispatch_runtime.dispatch_one().await });
        if tokio::time::timeout(std::time::Duration::from_secs(30), reached.wait())
            .await
            .is_err()
        {
            let current_wake = db::OrchestratorWakeRepo::get_orchestrator_wake(&**db, &wake_id)
                .await
                .unwrap();
            let current_request =
                db::TaskCoordinationRepo::get_task_coordination_request(&**db, &request.id)
                    .await
                    .unwrap();
            panic!(
                "pre-Start dispatcher did not reach its barrier: wake={:?}, request={:?}, dispatch_finished={}",
                current_wake.map(|wake| (wake.state, wake.current_attempt)),
                current_request.map(|request| (request.state, request.execution_id)),
                dispatch.is_finished(),
            );
        }
        let current_task = TaskRepo::get_by_id(&**db, &task.id, false)
            .await
            .unwrap()
            .unwrap();
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&**db, &task.id)
            .await
            .unwrap()
            .unwrap();
        lifecycle_service
            .pause_task(
                &task.id,
                current_task.version,
                lifecycle.version,
                &api_types::ActorRef::Human(owner.clone()),
                Some("pause exact coordinator request before provider Start".into()),
                "review-triage-pre-start-pause",
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(30), release.wait())
                .await
                .is_ok(),
            "pre-Start dispatcher reached the gate but did not reach its release barrier"
        );

        assert!(matches!(
            dispatch.await.unwrap().unwrap(),
            DispatchOutcome::Retried
        ));

        let deferred_request =
            db::TaskCoordinationRepo::get_task_coordination_request(&**db, &request.id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            deferred_request.state,
            db::TaskCoordinationRequestState::Running,
            "a no-Start pause keeps the typed request recoverable"
        );
        assert_eq!(deferred_request.wake_id.as_deref(), Some(wake_id.as_str()));
        let execution_id = deferred_request
            .execution_id
            .as_deref()
            .expect("the failed pre-Start Execution remains linked")
            .to_owned();
        let deferred_wake = db::OrchestratorWakeRepo::get_orchestrator_wake(&**db, &wake_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(deferred_wake.state, db::OrchestratorWakeState::Pending);
        assert_eq!(deferred_wake.current_attempt, None);
        let (attempt_state, attempt_execution_id): (String, String) = sqlx::query_as(
            "SELECT state,execution_id FROM orchestrator_wake_execution
             WHERE wake_id=? AND attempt_number=1",
        )
        .bind(&wake_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(attempt_state, "failed");
        assert_eq!(attempt_execution_id, execution_id);
        let execution = ExecutionRepo::get_by_id(&**db, &execution_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(execution.status, ExecutionStatus::Failed);
        assert!(DomainEventRepo::get_event_by_dedupe(
            &**db,
            &format!("execution.provider_start_attempted:{execution_id}")
        )
        .await
        .unwrap()
        .is_none());
        let turn_count_after: i64 =
            sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(turn_count_after, turn_count_before);

        let current_task = TaskRepo::get_by_id(&**db, &task.id, false)
            .await
            .unwrap()
            .unwrap();
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&**db, &task.id)
            .await
            .unwrap()
            .unwrap();
        lifecycle_service
            .resume_task(
                &task.id,
                current_task.version,
                lifecycle.version,
                &api_types::ActorRef::Human(owner.clone()),
                "review-triage-pre-start-resume",
            )
            .await
            .unwrap();
        let retry_at: String = sqlx::query_scalar(
            "SELECT available_at FROM orchestrator_wake WHERE id=? AND state='pending'",
        )
        .bind(&wake_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let retry_at = chrono::DateTime::parse_from_rfc3339(&retry_at).unwrap();
        let deferred_at = chrono::DateTime::parse_from_rfc3339(&deferred_wake.updated_at).unwrap();
        assert!(retry_at > deferred_at);
        // Simulate the retry delay elapsing in this disposable fixture by
        // advancing only availability; preserve the persisted request/attempt.
        let dispatch_now = now_rfc3339();
        let advanced = sqlx::query(
            "UPDATE orchestrator_wake SET available_at=? WHERE id=? AND state='pending'",
        )
        .bind(&dispatch_now)
        .bind(&wake_id)
        .execute(db.pool())
        .await
        .unwrap();
        assert_eq!(advanced.rows_affected(), 1);
        let resumed_dispatch_runtime = Arc::new(fixture.runtime.clone());
        let resumed_dispatch =
            tokio::spawn(async move { resumed_dispatch_runtime.dispatch_one().await });
        if tokio::time::timeout(std::time::Duration::from_secs(30), reached.wait())
            .await
            .is_err()
        {
            let current_wake = db::OrchestratorWakeRepo::get_orchestrator_wake(&**db, &wake_id)
                .await
                .unwrap();
            let current_request =
                db::TaskCoordinationRepo::get_task_coordination_request(&**db, &request.id)
                    .await
                    .unwrap();
            panic!(
                "resumed pre-Start dispatcher did not reach its barrier: wake={:?}, request={:?}, dispatch_finished={}",
                current_wake.map(|wake| (wake.state, wake.current_attempt)),
                current_request.map(|request| (request.state, request.execution_id)),
                resumed_dispatch.is_finished(),
            );
        }
        let resumed_request =
            db::TaskCoordinationRepo::get_task_coordination_request(&**db, &request.id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            resumed_request.state,
            db::TaskCoordinationRequestState::Running
        );
        assert_eq!(resumed_request.wake_id.as_deref(), Some(wake_id.as_str()));
        let resumed_execution_id = resumed_request
            .execution_id
            .as_deref()
            .expect("resume admits a fresh technical attempt for the same request")
            .to_owned();
        assert_ne!(resumed_execution_id, execution_id);
        let resumed_wake = db::OrchestratorWakeRepo::get_orchestrator_wake(&**db, &wake_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(resumed_wake.state, db::OrchestratorWakeState::Leased);
        assert_eq!(resumed_wake.current_attempt, Some(2));
        let second_attempt_state: String = sqlx::query_scalar(
            "SELECT state FROM orchestrator_wake_execution
             WHERE wake_id=? AND attempt_number=2",
        )
        .bind(&wake_id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!(second_attempt_state, "start_requested");
        let current_task = TaskRepo::get_by_id(&**db, &task.id, false)
            .await
            .unwrap()
            .unwrap();
        let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&**db, &task.id)
            .await
            .unwrap()
            .unwrap();
        lifecycle_service
            .pause_task(
                &task.id,
                current_task.version,
                lifecycle.version,
                &api_types::ActorRef::Human(owner.clone()),
                Some("pause same request before retry Start".into()),
                "review-triage-pre-start-pause-again",
            )
            .await
            .unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(30), release.wait())
                .await
                .is_ok(),
            "resumed pre-Start dispatcher reached its gate but not the release barrier"
        );
        assert!(matches!(
            resumed_dispatch.await.unwrap().unwrap(),
            DispatchOutcome::Retried
        ));
        let final_request =
            db::TaskCoordinationRepo::get_task_coordination_request(&**db, &request.id)
                .await
                .unwrap()
                .unwrap();
        assert_eq!(
            final_request.state,
            db::TaskCoordinationRequestState::Running
        );
        assert_eq!(final_request.wake_id.as_deref(), Some(wake_id.as_str()));
        assert_eq!(
            final_request.execution_id.as_deref(),
            Some(resumed_execution_id.as_str())
        );
        let final_wake = db::OrchestratorWakeRepo::get_orchestrator_wake(&**db, &wake_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(final_wake.state, db::OrchestratorWakeState::Pending);
        assert_eq!(final_wake.current_attempt, None);
        let attempt_states: Vec<(i64, String)> = sqlx::query_as(
            "SELECT attempt_number,state FROM orchestrator_wake_execution
             WHERE wake_id=? ORDER BY attempt_number",
        )
        .bind(&wake_id)
        .fetch_all(db.pool())
        .await
        .unwrap();
        assert_eq!(
            attempt_states,
            vec![(1, "failed".to_owned()), (2, "failed".to_owned())]
        );
        assert!(DomainEventRepo::get_event_by_dedupe(
            &**db,
            &format!("execution.provider_start_attempted:{resumed_execution_id}")
        )
        .await
        .unwrap()
        .is_none());
        let final_turn_count: i64 =
            sqlx::query_scalar("SELECT turn_count FROM task_coordination WHERE task_id=?")
                .bind(&task.id)
                .fetch_one(db.pool())
                .await
                .unwrap();
        assert_eq!(final_turn_count, turn_count_before);
        return;
    }
    let role = TaskRoleRepo::get_by_id(&**db, &fixture.task_role_id)
        .await
        .unwrap()
        .unwrap();
    let members = RoleMembershipRepo::list_by_role(&**db, &role.id, false)
        .await
        .unwrap();
    let policy = TaskRoleOrchestratorPolicy::parse(&role.policy_json).unwrap();
    let mut scoped_wake = fixture.wake.clone();
    scoped_wake.work_unit_id = Some(implementation.id.clone());
    let scoped = fixture
        .runtime
        .build_context(&scoped_wake, &role, &members, &policy, None)
        .await
        .unwrap();
    assert!(scoped.get("review_findings").is_none());
    assert!(scoped.get("pending_review_findings").is_none());
    assert!(serde_json::from_value::<OrchestratorResponse>(json!({
        "actions": [],
        "review_findings": [],
    }))
    .is_err());
    let mut typed_wake = fixture.wake.clone();
    typed_wake.event_id = request_event_id;
    typed_wake.event_sequence = request_event_sequence;
    let typed = fixture
        .runtime
        .build_context(&typed_wake, &role, &members, &policy, Some(&request))
        .await
        .unwrap();
    assert_eq!(typed["review_findings"]["report_artifact_id"], report.id);
    let prompt = coordination
        .render_turn_prompt(&request, &typed)
        .await
        .unwrap();
    assert!(prompt.contains("exactly one decision"));
    assert!(prompt.contains("no `actions`"));
    assert!(!prompt.contains("\"work_units\""));
    request = start_review_request(
        &coordination,
        db,
        &request,
        &fixture.wake.id,
        &fixture.execution.id,
    )
    .await;
    let mut mixed_output: serde_json::Value = serde_json::from_str(&review_findings_output(
        &request,
        &crate::task_rework::ReviewFindingsDecision {
            authorization_version: authorized.coordination.authorization_version,
            review_report_artifact_id: report.id.clone(),
            report_digest: report.digest.clone().unwrap(),
            disposition: api_types::TaskReviewDisposition::Human,
            rationale: "Needs a Human decision".into(),
            instructions: "Choose whether the evidence gap is acceptable".into(),
        },
    ))
    .unwrap();
    mixed_output["actions"] = json!([]);
    let mut mixed_execution = fixture.execution.clone();
    mixed_execution.summary = Some(mixed_output.to_string());
    assert!(coordination
        .complete_turn(&request, &mixed_execution)
        .await
        .is_err());
    let pause_decision = crate::task_rework::ReviewFindingsDecision {
        authorization_version: authorized.coordination.authorization_version,
        review_report_artifact_id: report.id.clone(),
        report_digest: report.digest.clone().unwrap(),
        disposition: api_types::TaskReviewDisposition::Human,
        rationale: "Pause fence check".into(),
        instructions: "Wait for dispatch to resume".into(),
    };
    let current_task = TaskRepo::get_by_id(&**db, &task.id, false)
        .await
        .unwrap()
        .unwrap();
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&**db, &task.id)
        .await
        .unwrap()
        .unwrap();
    lifecycle_service
        .pause_task(
            &task.id,
            current_task.version,
            lifecycle.version,
            &api_types::ActorRef::Human(owner.clone()),
            Some("exercise the exact ReviewFindings pause fence".into()),
            "review-triage-fixture-pause",
        )
        .await
        .unwrap();
    let paused =
        submit_review_findings(&coordination, &request, &fixture.execution, &pause_decision)
            .await
            .expect_err("writer pause fence preserves the completed response");
    assert!(matches!(
        paused,
        crate::ServiceError::Coordination {
            code: "coordination.paused",
            ..
        }
    ));
    let current_task = TaskRepo::get_by_id(&**db, &task.id, false)
        .await
        .unwrap()
        .unwrap();
    let lifecycle = db::TaskLifecycleRepo::get_task_lifecycle(&**db, &task.id)
        .await
        .unwrap()
        .unwrap();
    lifecycle_service
        .resume_task(
            &task.id,
            current_task.version,
            lifecycle.version,
            &api_types::ActorRef::Human(owner.clone()),
            "review-triage-fixture-resume",
        )
        .await
        .unwrap();
    for _ in 0..extra_rechecks {
        let decision = crate::task_rework::ReviewFindingsDecision {
            authorization_version: authorized.coordination.authorization_version,
            review_report_artifact_id: report.id.clone(),
            report_digest: report.digest.clone().unwrap(),
            disposition: api_types::TaskReviewDisposition::Recheck,
            rationale: "Check the exact acceptance evidence again".into(),
            instructions: "Independently verify the original delivery evidence".into(),
        };
        submit_review_findings(&coordination, &request, &fixture.execution, &decision)
            .await
            .unwrap();
        rework.process_pending_automatic_rework(10).await.unwrap();
        let id: String = sqlx::query_scalar(
            "SELECT review_work_unit_id FROM task_review_triage WHERE review_report_artifact_id=?",
        )
        .bind(&report.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        let next = WorkUnitRepo::get_by_id(&**db, &id).await.unwrap().unwrap();
        let (_,next_report)=units.submit_human_review(task_service,&next.id,&reviewer,next.version,serde_json::from_value(json!({"verdict":"request_changes","summary":"The acceptance evidence remains absent","criteria":["acceptance evidence"],"findings":["No acceptance evidence was produced"]})).unwrap()).await.unwrap();
        let event_id: String = sqlx::query_scalar(
            "SELECT id FROM domain_event WHERE event_type='artifact.created' AND entity_id=?",
        )
        .bind(&next_report.id)
        .fetch_one(db.pool())
        .await
        .unwrap();
        rework
            .process_domain_event(
                &DomainEventRepo::get_event(&**db, &event_id)
                    .await
                    .unwrap()
                    .unwrap(),
            )
            .await
            .unwrap();
        report = next_report;
        request = create_review_request(&coordination, &task.id, &report.id).await;
        request = start_review_request(
            &coordination,
            db,
            &request,
            &fixture.wake.id,
            &fixture.execution.id,
        )
        .await;
        let recheck_context = rework
            .exact_review_findings_context(&request)
            .await
            .unwrap();
        assert_eq!(recheck_context["review_work_unit_id"], next.id);
        assert_eq!(recheck_context["review_scope"], next.scope);
    }
    // Until the coordinator decides, the automatic pair must remain absent.
    rework.process_pending_automatic_rework(10).await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM task_rework_receipt WHERE task_id=?")
            .bind(&task.id)
            .fetch_one(db.pool())
            .await
            .unwrap(),
        0
    );
    let decision = crate::task_rework::ReviewFindingsDecision {
        authorization_version: authorized.coordination.authorization_version,
        review_report_artifact_id: report.id.clone(),
        report_digest: report.digest.clone().unwrap(),
        disposition,
        rationale: "Checked the requested evidence against the exact scope".into(),
        instructions: "Produce or independently verify the missing acceptance evidence".into(),
    };
    let mut wrong = decision.clone();
    wrong.report_digest = "0".repeat(64);
    assert!(
        submit_review_findings(&coordination, &request, &fixture.execution, &wrong)
            .await
            .is_err()
    );
    sqlx::query("UPDATE task_coordination SET team_stale=1 WHERE task_id=?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        submit_review_findings(&coordination, &request, &fixture.execution, &decision)
            .await
            .is_err()
    );
    sqlx::query("UPDATE task_coordination SET team_stale=0 WHERE task_id=?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .unwrap();
    submit_review_findings(&coordination, &request, &fixture.execution, &decision)
        .await
        .unwrap();
    submit_review_findings(&coordination, &request, &fixture.execution, &decision)
        .await
        .expect("exact retry is effect-free");
    let mut changed = decision;
    changed.instructions = "Different decision".into();
    assert!(
        submit_review_findings(&coordination, &request, &fixture.execution, &changed)
            .await
            .is_err()
    );
    rework.process_pending_automatic_rework(10).await.unwrap();
    let (state,recheck):(String,Option<String>)=sqlx::query_as("SELECT i.state,t.review_work_unit_id FROM task_rework_automation_intent i JOIN task_review_triage t ON t.review_report_artifact_id=i.review_report_artifact_id WHERE i.review_report_artifact_id=?").bind(&report.id).fetch_one(db.pool()).await.unwrap();
    match disposition {
        api_types::TaskReviewDisposition::Correct => {
            assert_eq!(state, "completed");
            let round:i64=sqlx::query_scalar("SELECT rework_round FROM task_rework_automation_intent WHERE review_report_artifact_id=?").bind(&report.id).fetch_one(db.pool()).await.unwrap();
            assert_eq!(round, extra_rechecks as i64 + 1);
        }
        api_types::TaskReviewDisposition::Human => assert_eq!(state, "blocked"),
        api_types::TaskReviewDisposition::Recheck => {
            assert_eq!(state, "cancelled");
            let reassessment = WorkUnitRepo::get_by_id(&**db, &recheck.unwrap())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(reassessment.assigned_actor, Some(ActorRef::Human(reviewer)));
            let exact:String=sqlx::query_scalar("SELECT result_execution_id FROM work_unit_review_subject WHERE review_work_unit_id=?").bind(&reassessment.id).fetch_one(db.pool()).await.unwrap();
            assert_eq!(exact, delivery_id);
        }
    }
    let next_context = fixture
        .runtime
        .build_context(&fixture.wake, &role, &members, &policy, None)
        .await
        .unwrap();
    assert!(next_context.get("previous_review_decisions").is_none());
    let original = CollaborationRepo::get_artifact(&**db, &report.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        original.content, report.content,
        "coordinator triage never changes a verdict"
    );
    assert_eq!(
        rework.list_review_triage(&task.id).await.unwrap().len(),
        extra_rechecks + 1
    );
}

#[tokio::test]
async fn confirmed_review_findings_create_one_exact_correction_pair() {
    triage_fixture(api_types::TaskReviewDisposition::Correct, 0, false).await;
}

#[tokio::test]
async fn disputed_review_findings_create_a_new_review_of_the_same_result() {
    triage_fixture(api_types::TaskReviewDisposition::Recheck, 0, false).await;
}

#[tokio::test]
async fn coordinator_can_request_human_help_without_approving_the_result() {
    triage_fixture(api_types::TaskReviewDisposition::Human, 0, false).await;
}

#[tokio::test]
async fn correction_round_ten_is_persisted_after_nine_independent_reassessments() {
    triage_fixture(api_types::TaskReviewDisposition::Correct, 9, false).await;
}

#[tokio::test]
async fn standalone_review_findings_pause_before_start_defers_same_request_and_wake() {
    triage_fixture(api_types::TaskReviewDisposition::Human, 0, true).await;
}
