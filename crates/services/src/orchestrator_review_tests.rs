use super::tests::completed_pr6_execution_fixture;
use super::*;
use crate::collaboration_service::CollaborationActorSource;
use db::{
    CreateProjectMember, CreateRoleMembership, CreateTaskRole, ProjectMemberRepo,
    RoleMembershipRepo, TaskRoleRepo, WorkUnitRepo,
};

async fn triage_fixture(disposition: api_types::TaskReviewDisposition, extra_rechecks: usize) {
    let fixture = completed_pr6_execution_fixture(Vec::new()).await;
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
    let pending = rework.pending_triage_context(&task.id).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["reviewed_result"]["execution_id"], delivery_id);
    assert_eq!(
        pending[0]["reviewed_result"]["original_scope"],
        implementation.scope
    );
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
        .build_context(&scoped_wake, &role, &members, &policy)
        .await
        .unwrap();
    assert_eq!(scoped["pending_review_findings"], json!([]));
    assert_eq!(scoped["previous_review_decisions"], json!([]));
    for _ in 0..extra_rechecks {
        rework
            .record_review_triage(
                &fixture.execution.id,
                crate::task_rework::ReviewFindingsDecision {
                    authorization_version: authorized.coordination.authorization_version,
                    review_report_artifact_id: report.id.clone(),
                    report_digest: report.digest.clone().unwrap(),
                    disposition: api_types::TaskReviewDisposition::Recheck,
                    rationale: "Check the exact acceptance evidence again".into(),
                    instructions: "Independently verify the original delivery evidence".into(),
                },
            )
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
    assert!(rework
        .record_review_triage(&fixture.execution.id, wrong)
        .await
        .is_err());
    sqlx::query("UPDATE task_coordination SET team_stale=1 WHERE task_id=?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(rework
        .record_review_triage(&fixture.execution.id, decision.clone())
        .await
        .is_err());
    sqlx::query("UPDATE task_coordination SET team_stale=0 WHERE task_id=?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .unwrap();
    rework
        .record_review_triage(&fixture.execution.id, decision.clone())
        .await
        .unwrap();
    rework
        .record_review_triage(&fixture.execution.id, decision.clone())
        .await
        .expect("exact retry is effect-free");
    let mut changed = decision;
    changed.instructions = "Different decision".into();
    assert!(rework
        .record_review_triage(&fixture.execution.id, changed)
        .await
        .is_err());
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
        .build_context(&fixture.wake, &role, &members, &policy)
        .await
        .unwrap();
    assert_eq!(
        next_context["previous_review_decisions"]
            .as_array()
            .unwrap()
            .len(),
        (extra_rechecks + 1).min(8)
    );
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
    triage_fixture(api_types::TaskReviewDisposition::Correct, 0).await;
}

#[tokio::test]
async fn disputed_review_findings_create_a_new_review_of_the_same_result() {
    triage_fixture(api_types::TaskReviewDisposition::Recheck, 0).await;
}

#[tokio::test]
async fn coordinator_can_request_human_help_without_approving_the_result() {
    triage_fixture(api_types::TaskReviewDisposition::Human, 0).await;
}

#[tokio::test]
async fn correction_round_ten_is_persisted_after_nine_independent_reassessments() {
    triage_fixture(api_types::TaskReviewDisposition::Correct, 9).await;
}
