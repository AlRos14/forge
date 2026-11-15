//! Phase 1 persistence of the coordinator-verification-and-pr-loop plan:
//! V171 (validation check catalog), V172 (coordination request kinds, validation
//! triage, coordinator replans) and V174 (human decision requests).

use super::{
    copy_migrations_through, page, seed_project, seed_task, seed_user, seed_workspace_for_task,
    sqlite_db,
};
use crate::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, run_migrations_from,
    AnswerHumanDecision, AuthorityEffect, CoordinatorReplanState, CreateCoordinatorReplan,
    CreateEvidence, CreateHumanDecisionRequest, CreateTaskRole, CreateValidationCheckRequest,
    CreateValidationComparison, CreateValidationRun, DbError, FinishValidationRun,
    HumanDecisionRepo, HumanDecisionSourceKind, HumanDecisionState,
    PublishValidationCheckDefinition, SqliteDb, TaskCoordinatorReplanRepo, TaskRoleRepo,
    TaskValidationTriage, TaskValidationTriageRepo, TransitionCoordinatorReplan,
    TransitionValidationCheckRequest, ValidationCatalogRepo, ValidationCheckDigestInput,
    ValidationCheckRequestRepo, ValidationCheckRequestState, ValidationCheckRequestTrigger,
    ValidationCheckState, ValidationCheckSubjectKind, ValidationComparisonClass,
    ValidationComparisonRepo, ValidationRunOrigin, ValidationRunRepo, ValidationRunStatus,
    ValidationTriageDisposition, MAX_COORDINATOR_REPLANS_PER_TASK,
};
use sha2::Digest as _;

fn digest64(seed: &str) -> String {
    hex::encode(sha2::Sha256::digest(seed.as_bytes()))
}

fn v17x_event(task_id: &str, event_type: &str, entity_id: &str) -> crate::CreateDomainEvent {
    crate::CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: "validation_run".to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: entity_id.to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: Some(format!("{event_type}:{entity_id}")),
        payload_json: "{}".to_owned(),
        created_at: now_rfc3339(),
    }
}

struct Fixture {
    db: SqliteDb,
    user_id: String,
    project_id: String,
    repo_id: String,
    task_id: String,
    workspace_id: String,
    /// A running `work_unit_plan` request of the authorized coordination (version 1).
    request_id: String,
    execution_id: String,
}

/// Authorized `prepare_and_execute` coordination whose envelope pins the given
/// `validation_checks` JSON array.
async fn fixture(validation_checks: serde_json::Value) -> Fixture {
    let db = sqlite_db().await;
    let user_id = seed_user(&db).await;
    let project_id = seed_project(&db, "v17x fixture", Some(user_id.clone())).await;
    let now = now_rfc3339();
    let repo_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, created_at, updated_at)
         VALUES (?, ?, 'repo', 'https://example.invalid/repo.git', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("repo");
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "in_progress".to_owned(),
        "v17x task",
    )
    .await;
    let workspace_id = seed_workspace_for_task(&db, &task_id, &repo_id).await;
    let task_role_id = new_uuid_v4();
    TaskRoleRepo::create(
        &db,
        CreateTaskRole {
            id: task_role_id.clone(),
            task_id: task_id.clone(),
            role: "coordinator".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("coordinator TaskRole");
    let envelope = serde_json::json!({"schema_version": 2, "validation_checks": validation_checks});
    sqlx::query(
        "UPDATE task_coordination SET mode = 'prepare_and_execute', coordinator_task_role_id = ?,
            coordinator_actor_kind = 'agent', coordinator_actor_id = 'agent-v17x',
            authorization_version = 1, envelope_json = ?, envelope_digest = ?,
            team_digest = ?, requested_by = ? WHERE task_id = ?",
    )
    .bind(&task_role_id)
    .bind(envelope.to_string())
    .bind(digest64("envelope"))
    .bind(digest64("team"))
    .bind(&user_id)
    .bind(&task_id)
    .execute(db.pool())
    .await
    .expect("authorize coordination");
    // Changing authority marks the team stale; the fixture models a settled team.
    sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("settle team");
    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (id, task_id, status, created_at, updated_at)
         VALUES (?, ?, 'completed', ?, ?)",
    )
    .bind(&execution_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("completed Execution");
    let request_id = new_uuid_v4();
    insert_request(
        &db,
        &task_id,
        &request_id,
        "running",
        "work_unit_plan",
        None,
        None,
    )
    .await
    .expect("work_unit_plan request");
    Fixture {
        db,
        user_id,
        project_id,
        repo_id,
        task_id,
        workspace_id,
        request_id,
        execution_id,
    }
}

async fn insert_request(
    db: &SqliteDb,
    task_id: &str,
    id: &str,
    state: &str,
    kind: &str,
    subject: Option<(&str, &str, &str)>,
    resumes: Option<&str>,
) -> std::result::Result<sqlx::sqlite::SqliteQueryResult, sqlx::Error> {
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json, objective_digest,
            request_digest, idempotency_key, state, version, created_at, updated_at, kind,
            source_subject_kind, source_subject_id, source_subject_digest, resumes_request_id
         ) VALUES (?, ?, 1, 'prepare_and_execute', '{}', ?, ?, ?, ?, 1, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(task_id)
    .bind(digest64("objective"))
    .bind(digest64("request"))
    .bind(format!("key-{id}"))
    .bind(state)
    .bind(&now)
    .bind(&now)
    .bind(kind)
    .bind(subject.map(|value| value.0))
    .bind(subject.map(|value| value.1))
    .bind(subject.map(|value| value.2))
    .bind(resumes)
    .execute(db.pool())
    .await
}

fn pin(check_key: &str, definition_digest: &str, trigger: &str) -> serde_json::Value {
    serde_json::json!({
        "check_key": check_key,
        "definition_digest": definition_digest,
        "trigger": trigger,
    })
}

async fn publish(f: &Fixture, check_key: &str, command: &str) -> crate::ValidationCheckDefinition {
    ValidationCatalogRepo::publish_validation_check_definition(
        &f.db,
        PublishValidationCheckDefinition {
            id: new_uuid_v4(),
            project_id: f.project_id.clone(),
            repo_id: None,
            check_key: check_key.to_owned(),
            command: command.to_owned(),
            timeout_seconds: 60,
            env_policy: "captured_default".to_owned(),
            created_by: f.user_id.clone(),
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("definition publishes")
}

async fn set_pins(f: &Fixture, pins: serde_json::Value) {
    let envelope = serde_json::json!({"schema_version": 2, "validation_checks": pins});
    sqlx::query("UPDATE task_coordination SET envelope_json = ?, team_stale = 0 WHERE task_id = ?")
        .bind(envelope.to_string())
        .bind(&f.task_id)
        .execute(f.db.pool())
        .await
        .expect("repin envelope");
}

fn check_request_input(
    f: &Fixture,
    definition: &crate::ValidationCheckDefinition,
    key: &str,
    baseline: bool,
) -> CreateValidationCheckRequest {
    CreateValidationCheckRequest {
        id: new_uuid_v4(),
        task_id: f.task_id.clone(),
        authorization_version: 1,
        check_definition_id: definition.id.clone(),
        definition_digest: definition.definition_digest.clone(),
        trigger_kind: ValidationCheckRequestTrigger::CoordinatorRequest,
        subject_kind: ValidationCheckSubjectKind::WorkUnitResult,
        subject_id: "work-unit-1".to_owned(),
        subject_version: 3,
        workspace_id: f.workspace_id.clone(),
        commit_sha: "a".repeat(40),
        workspace_snapshot_digest: digest64("snapshot"),
        baseline,
        requested_by_request_id: Some(f.request_id.clone()),
        requested_by_execution_id: None,
        reason: Some("services touched".to_owned()),
        idempotency_key: key.to_owned(),
        created_at: now_rfc3339(),
    }
}

fn transition(
    request: &crate::ValidationCheckRequest,
    to_state: ValidationCheckRequestState,
) -> TransitionValidationCheckRequest {
    let terminal = to_state.is_terminal();
    TransitionValidationCheckRequest {
        id: request.id.clone(),
        expected_version: request.version,
        to_state,
        candidate_run_id: request.candidate_run_id.clone(),
        baseline_commit_sha: None,
        failure_code: (to_state == ValidationCheckRequestState::FailedToStart)
            .then(|| "validation.baseline_unavailable".to_owned()),
        updated_at: now_rfc3339(),
        completed_at: terminal.then(now_rfc3339),
    }
}

/// Start a catalog run through the repository, then finish it with exact Evidence.
async fn run_catalog_check(
    f: &Fixture,
    request: &crate::ValidationCheckRequest,
    status: ValidationRunStatus,
    exit_code: i32,
) -> crate::ValidationRun {
    let run_id = new_uuid_v4();
    let started_at = now_rfc3339();
    let summary = serde_json::json!({"definition_digest": request.definition_digest}).to_string();
    let config_digest = digest64(&summary);
    let input = CreateValidationRun {
        id: run_id.clone(),
        task_id: f.task_id.clone(),
        work_unit_id: None,
        caused_by_execution_id: None,
        check_identity: format!("catalog:check@{}", &request.definition_digest[..16]),
        command: "cargo test".to_owned(),
        config_summary_json: summary,
        config_digest: config_digest.clone(),
        workspace_id: request.workspace_id.clone(),
        commit_sha: request.commit_sha.clone(),
        workspace_snapshot_digest: request.workspace_snapshot_digest.clone(),
        idempotency_key: format!("run:{}", request.id),
        catalog_check_id: Some(request.check_definition_id.clone()),
        definition_digest: Some(request.definition_digest.clone()),
        run_origin: ValidationRunOrigin::CoordinatorRequest,
        check_request_id: Some(request.id.clone()),
        started_at: started_at.clone(),
        created_at: started_at.clone(),
        updated_at: started_at.clone(),
    };
    let started = ValidationRunRepo::start_validation_run(
        &f.db,
        input.clone(),
        v17x_event(&f.task_id, "validation_run.started", &run_id),
    )
    .await
    .expect("catalog run starts");
    assert_eq!(
        started.validation_run.run_origin,
        ValidationRunOrigin::CoordinatorRequest
    );
    assert_eq!(
        started.validation_run.check_request_id.as_deref(),
        Some(request.id.as_str())
    );
    // An idempotent replay of the stored key must bypass the catalog guard.
    let replay = ValidationRunRepo::start_validation_run(
        &f.db,
        input.clone(),
        v17x_event(&f.task_id, "validation_run.started", &run_id),
    )
    .await
    .expect("catalog run start is idempotent");
    assert!(replay.event.is_none());

    let owner = new_uuid_v4();
    let finished_at = now_rfc3339();
    assert!(ValidationRunRepo::claim_validation_run(
        &f.db,
        &run_id,
        &owner,
        &started_at,
        "9999-01-01T00:00:00Z",
    )
    .await
    .expect("claim"));
    let evidence_id = new_uuid_v4();
    let content = serde_json::json!({
        "validation_run_id": run_id,
        "task_id": f.task_id,
        "check_identity": input.check_identity,
        "command": input.command,
        "config_digest": config_digest,
        "workspace_id": input.workspace_id,
        "commit_sha": input.commit_sha,
        "workspace_snapshot_digest": input.workspace_snapshot_digest,
        "status": status.to_string(),
        "exit_code": exit_code,
        "started_at": started_at,
        "finished_at": finished_at,
    })
    .to_string();
    let completed = ValidationRunRepo::finish_validation_run(
        &f.db,
        FinishValidationRun {
            id: run_id.clone(),
            claim_owner: owner,
            status,
            exit_code: Some(exit_code),
            finished_at: finished_at.clone(),
            logs_ref: format!("validation-evidence://{evidence_id}"),
            evidence: vec![CreateEvidence {
                id: evidence_id,
                task_id: f.task_id.clone(),
                validation_run_id: run_id.clone(),
                evidence_key: "check:0".to_owned(),
                kind: "deterministic_check_output".to_owned(),
                digest: digest64(&content),
                content_json: content,
                created_at: finished_at,
            }],
            validation_report: None,
            events: vec![],
        },
    )
    .await
    .expect("catalog run finishes");
    completed.validation_run
}

#[tokio::test]
async fn v171_catalog_versions_are_exact_immutable_and_retire_the_previous_active() {
    let f = fixture(serde_json::json!([])).await;
    let first = publish(&f, "services-tests", "cargo test -p services").await;
    assert_eq!(first.version, 1);
    assert_eq!(first.state, ValidationCheckState::Active);
    assert_eq!(
        first.definition_digest,
        crate::validation_check_definition_digest(&ValidationCheckDigestInput {
            check_key: "services-tests".to_owned(),
            version: 1,
            command: "cargo test -p services".to_owned(),
            timeout_seconds: 60,
            env_policy: "captured_default".to_owned(),
        })
    );

    let second = publish(&f, "services-tests", "cargo test -p services --lib").await;
    assert_eq!(second.version, 2);
    assert_ne!(second.definition_digest, first.definition_digest);
    let first_now = ValidationCatalogRepo::get_validation_check_definition(&f.db, &first.id)
        .await
        .expect("definition read")
        .expect("definition exists");
    assert_eq!(first_now.state, ValidationCheckState::Retired);
    assert!(first_now.retired_at.is_some());
    let active = ValidationCatalogRepo::get_active_validation_check_definition(
        &f.db,
        &f.project_id,
        None,
        "services-tests",
    )
    .await
    .expect("active lookup")
    .expect("one active version");
    assert_eq!(active.id, second.id);

    // Retiring a definition that is no longer active is a typed conflict.
    let stale = ValidationCatalogRepo::retire_validation_check_definition(
        &f.db,
        &first.id,
        ValidationCheckState::Active,
        &now_rfc3339(),
    )
    .await;
    assert!(matches!(stale, Err(DbError::VersionConflict)));
    let missing = ValidationCatalogRepo::retire_validation_check_definition(
        &f.db,
        &new_uuid_v4(),
        ValidationCheckState::Active,
        &now_rfc3339(),
    )
    .await;
    assert!(matches!(missing, Err(DbError::NotFound)));

    let active_only = ValidationCatalogRepo::list_validation_check_definitions(
        &f.db,
        &f.project_id,
        None,
        Some(ValidationCheckState::Active),
        &page(10),
    )
    .await
    .expect("list");
    assert_eq!(active_only.items.len(), 1);
    let all = ValidationCatalogRepo::list_validation_check_definitions(
        &f.db,
        &f.project_id,
        None,
        None,
        &page(10),
    )
    .await
    .expect("list");
    assert_eq!(all.items.len(), 2);

    // Immutability guards: content edits, reactivation and deletes are rejected.
    for statement in [
        "UPDATE validation_check_definition SET command = 'rm -rf /' WHERE id = ?",
        "UPDATE validation_check_definition SET state = 'active', retired_at = NULL WHERE id = ?",
        "DELETE FROM validation_check_definition WHERE id = ?",
    ] {
        assert!(
            sqlx::query(statement)
                .bind(&first.id)
                .execute(f.db.pool())
                .await
                .is_err(),
            "{statement} must be rejected"
        );
    }
    // Shape CHECKs and the next-exact-version guard.
    for (key, version, timeout) in [
        ("Bad Key", 1, 60),
        ("fresh-key", 2, 60),
        ("fresh-key", 1, 0),
    ] {
        let rejected = sqlx::query(
            "INSERT INTO validation_check_definition (
                id, project_id, check_key, version, command, timeout_seconds, env_policy,
                definition_digest, state, created_by, created_at
             ) VALUES (?, ?, ?, ?, 'true', ?, 'captured_default', ?, 'active', ?, ?)",
        )
        .bind(new_uuid_v4())
        .bind(&f.project_id)
        .bind(key)
        .bind(version)
        .bind(timeout)
        .bind(digest64(key))
        .bind(&f.user_id)
        .bind(now_rfc3339())
        .execute(f.db.pool())
        .await;
        assert!(rejected.is_err(), "{key} v{version} timeout {timeout}");
    }
    // Only a Project owner or admin may author a definition.
    let outsider = seed_user(&f.db).await;
    let denied = ValidationCatalogRepo::publish_validation_check_definition(
        &f.db,
        PublishValidationCheckDefinition {
            id: new_uuid_v4(),
            project_id: f.project_id.clone(),
            repo_id: None,
            check_key: "other".to_owned(),
            command: "true".to_owned(),
            timeout_seconds: 5,
            env_policy: "captured_default".to_owned(),
            created_by: outsider,
            created_at: now_rfc3339(),
        },
    )
    .await;
    assert!(denied.is_err());
}

#[tokio::test]
async fn v171_check_requests_require_the_exact_envelope_pin_and_are_idempotent() {
    let f = fixture(serde_json::json!([])).await;
    let definition = publish(&f, "services-tests", "cargo test").await;

    // No pin: not authorized.
    let unpinned = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        check_request_input(&f, &definition, "unpinned", false),
        None,
    )
    .await;
    assert!(matches!(
        unpinned,
        Err(DbError::ValidationCheckNotAuthorized)
    ));

    // A pin whose trigger is after_integration does not authorize a coordinator request.
    set_pins(
        &f,
        serde_json::json!([pin(
            "services-tests",
            &definition.definition_digest,
            "after_integration"
        )]),
    )
    .await;
    let wrong_trigger = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        check_request_input(&f, &definition, "wrong-trigger", false),
        None,
    )
    .await;
    assert!(matches!(
        wrong_trigger,
        Err(DbError::ValidationCheckNotAuthorized)
    ));

    set_pins(
        &f,
        serde_json::json!([pin(
            "services-tests",
            &definition.definition_digest,
            "on_request"
        )]),
    )
    .await;
    let input = check_request_input(&f, &definition, "request-1", true);
    let created = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        input.clone(),
        Some(v17x_event(
            &f.task_id,
            "task.validation_check_requested",
            &input.id,
        )),
    )
    .await
    .expect("pinned check queues");
    assert!(created.created);
    assert!(created.event.is_some());
    assert_eq!(created.request.state, ValidationCheckRequestState::Queued);
    assert_eq!(created.request.version, 1);
    assert!(created.request.baseline);

    // Same key and subject replays; a different exact subject is a conflict.
    let mut replay_input = input.clone();
    replay_input.id = new_uuid_v4();
    let replay = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        replay_input.clone(),
        None,
    )
    .await
    .expect("replay");
    assert!(!replay.created);
    assert_eq!(replay.request.id, input.id);
    replay_input.subject_version += 1;
    let conflict =
        ValidationCheckRequestRepo::create_validation_check_request(&f.db, replay_input, None)
            .await;
    assert!(matches!(conflict, Err(DbError::IdempotencyConflict)));

    // Stale authorization version and a different digest are rejected.
    let mut stale_authorization = check_request_input(&f, &definition, "stale-auth", false);
    stale_authorization.authorization_version = 2;
    assert!(matches!(
        ValidationCheckRequestRepo::create_validation_check_request(
            &f.db,
            stale_authorization,
            None
        )
        .await,
        Err(DbError::ValidationCheckNotAuthorized)
    ));

    // Forward-only transitions guarded by expected_version.
    let mut stale_version = transition(&created.request, ValidationCheckRequestState::Running);
    stale_version.expected_version = 7;
    assert!(matches!(
        ValidationCheckRequestRepo::transition_validation_check_request(&f.db, stale_version, None)
            .await,
        Err(DbError::VersionConflict)
    ));
    let mut running_input = transition(&created.request, ValidationCheckRequestState::Running);
    running_input.baseline_commit_sha = Some("b".repeat(40));
    let running =
        ValidationCheckRequestRepo::transition_validation_check_request(&f.db, running_input, None)
            .await
            .expect("queued -> running")
            .request;
    assert_eq!(running.version, 2);
    assert_eq!(
        running.baseline_commit_sha.as_deref(),
        Some("b".repeat(40).as_str())
    );
    // running -> completed needs a terminal candidate run.
    assert!(
        ValidationCheckRequestRepo::transition_validation_check_request(
            &f.db,
            transition(&running, ValidationCheckRequestState::Completed),
            None,
        )
        .await
        .is_err()
    );
    // Identity columns never change.
    assert!(sqlx::query(
        "UPDATE validation_check_request SET workspace_id = ?, version = 99 WHERE id = ?"
    )
    .bind(new_uuid_v4())
    .bind(&input.id)
    .execute(f.db.pool())
    .await
    .is_err());
    let by_state = ValidationCheckRequestRepo::list_validation_check_requests_by_state(
        &f.db,
        ValidationCheckRequestState::Running,
        10,
    )
    .await
    .expect("runner poll");
    assert_eq!(by_state.len(), 1);
    // A cancelled request is terminal and cannot be revived.
    let cancelled = ValidationCheckRequestRepo::transition_validation_check_request(
        &f.db,
        transition(&running, ValidationCheckRequestState::Cancelled),
        None,
    )
    .await
    .expect("running -> cancelled")
    .request;
    assert!(
        ValidationCheckRequestRepo::transition_validation_check_request(
            &f.db,
            transition(&cancelled, ValidationCheckRequestState::Running),
            None,
        )
        .await
        .is_err()
    );

    // A retired definition makes a fresh request a stale pin.
    let newer = publish(&f, "services-tests", "cargo test --lib").await;
    assert_ne!(newer.id, definition.id);
    assert!(matches!(
        ValidationCheckRequestRepo::create_validation_check_request(
            &f.db,
            check_request_input(&f, &definition, "retired-pin", false),
            None,
        )
        .await,
        Err(DbError::ValidationCheckStalePin)
    ));
}

#[tokio::test]
async fn v171_catalog_run_comparison_and_completion_bind_the_exact_baseline_facts() {
    let f = fixture(serde_json::json!([])).await;
    let definition = publish(&f, "services-tests", "cargo test").await;
    set_pins(
        &f,
        serde_json::json!([pin(
            "services-tests",
            &definition.definition_digest,
            "on_request"
        )]),
    )
    .await;
    let created = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        check_request_input(&f, &definition, "baseline-request", true),
        None,
    )
    .await
    .expect("queue")
    .request;
    let mut to_running = transition(&created, ValidationCheckRequestState::Running);
    to_running.baseline_commit_sha = Some("c".repeat(40));
    let running =
        ValidationCheckRequestRepo::transition_validation_check_request(&f.db, to_running, None)
            .await
            .expect("running")
            .request;

    // Baseline is never a ValidationRun (decision C2) and provenance is all-or-nothing.
    for (origin, catalog, digest, request) in [
        (ValidationRunOrigin::Baseline, true, true, true),
        (ValidationRunOrigin::CoordinatorRequest, false, false, false),
        (ValidationRunOrigin::LegacyCommand, true, true, true),
        (ValidationRunOrigin::CoordinatorRequest, true, true, false),
    ] {
        let rejected = ValidationRunRepo::start_validation_run(
            &f.db,
            CreateValidationRun {
                id: new_uuid_v4(),
                task_id: f.task_id.clone(),
                work_unit_id: None,
                caused_by_execution_id: None,
                check_identity: "catalog:probe".to_owned(),
                command: "true".to_owned(),
                config_summary_json: "{}".to_owned(),
                config_digest: digest64("{}"),
                workspace_id: running.workspace_id.clone(),
                commit_sha: running.commit_sha.clone(),
                workspace_snapshot_digest: running.workspace_snapshot_digest.clone(),
                idempotency_key: new_uuid_v4(),
                catalog_check_id: catalog.then(|| running.check_definition_id.clone()),
                definition_digest: digest.then(|| running.definition_digest.clone()),
                run_origin: origin,
                check_request_id: request.then(|| running.id.clone()),
                started_at: now_rfc3339(),
                created_at: now_rfc3339(),
                updated_at: now_rfc3339(),
            },
            v17x_event(&f.task_id, "validation_run.started", &new_uuid_v4()),
        )
        .await;
        assert!(rejected.is_err(), "{origin:?} {catalog} {digest} {request}");
    }

    let candidate = run_catalog_check(&f, &running, ValidationRunStatus::Failed, 1).await;
    assert_eq!(candidate.status, ValidationRunStatus::Failed);
    assert_eq!(
        candidate.run_origin,
        ValidationRunOrigin::CoordinatorRequest
    );
    // A second run cannot claim the same request.
    let duplicate = ValidationRunRepo::start_validation_run(
        &f.db,
        CreateValidationRun {
            id: new_uuid_v4(),
            task_id: f.task_id.clone(),
            work_unit_id: None,
            caused_by_execution_id: None,
            check_identity: "catalog:probe".to_owned(),
            command: "true".to_owned(),
            config_summary_json: "{}".to_owned(),
            config_digest: digest64("{}"),
            workspace_id: running.workspace_id.clone(),
            commit_sha: running.commit_sha.clone(),
            workspace_snapshot_digest: running.workspace_snapshot_digest.clone(),
            idempotency_key: new_uuid_v4(),
            catalog_check_id: Some(running.check_definition_id.clone()),
            definition_digest: Some(running.definition_digest.clone()),
            run_origin: ValidationRunOrigin::CoordinatorRequest,
            check_request_id: Some(running.id.clone()),
            started_at: now_rfc3339(),
            created_at: now_rfc3339(),
            updated_at: now_rfc3339(),
        },
        v17x_event(&f.task_id, "validation_run.started", &new_uuid_v4()),
    )
    .await;
    assert!(duplicate.is_err());
    assert!(
        sqlx::query("UPDATE validation_run SET run_origin = 'legacy_command' WHERE id = ?")
            .bind(&candidate.id)
            .execute(f.db.pool())
            .await
            .is_err(),
        "catalog provenance is immutable"
    );

    // Bind the candidate run; completion needs the baseline comparison.
    let mut attach = transition(&running, ValidationCheckRequestState::Running);
    attach.candidate_run_id = Some(candidate.id.clone());
    let attached =
        ValidationCheckRequestRepo::transition_validation_check_request(&f.db, attach, None)
            .await
            .expect("attach candidate")
            .request;
    assert_eq!(
        attached.candidate_run_id.as_deref(),
        Some(candidate.id.as_str())
    );
    assert!(
        ValidationCheckRequestRepo::transition_validation_check_request(
            &f.db,
            transition(&attached, ValidationCheckRequestState::Completed),
            None,
        )
        .await
        .is_err(),
        "baseline=true completion requires the comparison"
    );

    let comparison_input =
        |baseline_status, baseline_exit, baseline_commit: &str| CreateValidationComparison {
            id: new_uuid_v4(),
            task_id: f.task_id.clone(),
            check_request_id: attached.id.clone(),
            candidate_run_id: candidate.id.clone(),
            candidate_output_digest: Some(digest64("candidate-output")),
            baseline_commit_sha: baseline_commit.to_owned(),
            baseline_snapshot_digest: digest64("baseline-snapshot"),
            baseline_provenance_json: r#"{"runner":"disposable-detached-worktree"}"#.to_owned(),
            baseline_status,
            baseline_exit_code: baseline_exit,
            baseline_output_digest: Some(digest64("baseline-output")),
            created_at: now_rfc3339(),
        };
    // The comparison must name the request's exact resolved base commit.
    assert!(ValidationComparisonRepo::create_validation_comparison(
        &f.db,
        comparison_input(ValidationRunStatus::Passed, Some(0), &"d".repeat(40)),
        None,
    )
    .await
    .is_err());
    // A failed baseline needs a non-zero exit code.
    assert!(ValidationComparisonRepo::create_validation_comparison(
        &f.db,
        comparison_input(ValidationRunStatus::Failed, Some(0), &"c".repeat(40)),
        None,
    )
    .await
    .is_err());
    let input = comparison_input(ValidationRunStatus::Passed, Some(0), &"c".repeat(40));
    let write = ValidationComparisonRepo::create_validation_comparison(&f.db, input.clone(), None)
        .await
        .expect("comparison");
    assert!(write.created);
    assert_eq!(
        write.comparison.classification,
        ValidationComparisonClass::Regression
    );
    assert!(!write.comparison.same_output_digest);
    assert_eq!(
        write.comparison.candidate_status,
        ValidationRunStatus::Failed
    );
    let replay = ValidationComparisonRepo::create_validation_comparison(&f.db, input, None)
        .await
        .expect("replay");
    assert!(!replay.created);
    assert_eq!(replay.comparison.id, write.comparison.id);
    for statement in [
        "UPDATE validation_comparison SET classification = 'fixed'",
        "DELETE FROM validation_comparison",
    ] {
        assert!(sqlx::query(statement).execute(f.db.pool()).await.is_err());
    }
    assert_eq!(
        ValidationComparisonClass::classify(
            ValidationRunStatus::Failed,
            ValidationRunStatus::Failed
        ),
        ValidationComparisonClass::PreExistingFailure
    );
    assert_eq!(
        ValidationComparisonClass::classify(
            ValidationRunStatus::Error,
            ValidationRunStatus::Passed
        ),
        ValidationComparisonClass::Inconclusive
    );

    let completed = ValidationCheckRequestRepo::transition_validation_check_request(
        &f.db,
        transition(&attached, ValidationCheckRequestState::Completed),
        None,
    )
    .await
    .expect("comparison present: completes")
    .request;
    assert_eq!(completed.state, ValidationCheckRequestState::Completed);
    assert!(completed.completed_at.is_some());
    let listed =
        ValidationComparisonRepo::list_validation_comparisons_by_task(&f.db, &f.task_id, &page(10))
            .await
            .expect("list");
    assert_eq!(listed.items.len(), 1);

    // validation_findings over the exact comparison (V172) round-trips through the repo.
    let findings_id = new_uuid_v4();
    let run_digest = digest64("run-digest");
    insert_request(
        &f.db,
        &f.task_id,
        &findings_id,
        "requested",
        "validation_findings",
        Some((
            "validation_comparison",
            write.comparison.id.as_str(),
            run_digest.as_str(),
        )),
        None,
    )
    .await
    .expect("regression comparison is a valid validation_findings source");
}

async fn legacy_baseline_db(version_limit: i64) -> (sqlx::SqlitePool, SqliteDb, tempfile::TempDir) {
    let migrations = tempfile::tempdir().expect("disposable migrations");
    copy_migrations_through(version_limit, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("baseline migrations");
    let db = SqliteDb::new(pool.clone());
    (pool, db, migrations)
}

async fn schema_names(pool: &sqlx::SqlitePool) -> std::collections::BTreeSet<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT type, name FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
    )
    .fetch_all(pool)
    .await
    .expect("schema names")
    .into_iter()
    .collect()
}

#[tokio::test]
async fn v171_migration_preserves_legacy_validation_runs_as_legacy_command() {
    let (pool, db, _migrations) = legacy_baseline_db(170).await;
    let user_id = seed_user(&db).await;
    let project_id = seed_project(&db, "V171 legacy", Some(user_id)).await;
    let now = now_rfc3339();
    let repo_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, created_at, updated_at)
         VALUES (?, ?, 'repo', 'https://example.invalid/repo.git', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("repo");
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "in_progress".to_owned(),
        "legacy",
    )
    .await;
    let workspace_id = seed_workspace_for_task(&db, &task_id, &repo_id).await;
    let run_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO validation_run (
            id, task_id, check_identity, command, config_summary_json, config_digest,
            workspace_id, commit_sha, workspace_snapshot_digest, idempotency_key, status,
            started_at, created_at, updated_at
         ) VALUES (?, ?, 'ci:0:legacy', 'cargo test', '{}', ?, ?, ?, ?, 'legacy-key', 'running', ?, ?, ?)",
    )
    .bind(&run_id)
    .bind(&task_id)
    .bind(digest64("{}"))
    .bind(&workspace_id)
    .bind("e".repeat(40))
    .bind(digest64("snapshot"))
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical run");
    let before = schema_names(&pool).await;

    run_migrations(&pool).await.expect("V171+ upgrade");
    run_migrations(&pool).await.expect("idempotent upgrade");

    let after = schema_names(&pool).await;
    assert!(
        before.is_subset(&after),
        "no schema object is lost: {:?}",
        before.difference(&after).collect::<Vec<_>>()
    );
    let run = ValidationRunRepo::get_validation_run(&db, &run_id)
        .await
        .expect("run query")
        .expect("historical run survives");
    assert_eq!(run.run_origin, ValidationRunOrigin::LegacyCommand);
    assert!(run.catalog_check_id.is_none());
    assert!(run.definition_digest.is_none());
    assert!(run.check_request_id.is_none());
    assert_eq!(run.check_identity, "ci:0:legacy");
    let violations: i64 = sqlx::query_scalar("SELECT count(*) FROM pragma_foreign_key_check")
        .fetch_one(&pool)
        .await
        .expect("foreign key audit");
    assert_eq!(violations, 0);
}

#[tokio::test]
async fn v172_migration_preserves_coordination_history_dependents_and_guards() {
    let (pool, db, _migrations) = legacy_baseline_db(170).await;
    let user_id = seed_user(&db).await;
    let project_id = seed_project(&db, "V172 history", Some(user_id)).await;
    let now = now_rfc3339();
    let repo_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, created_at, updated_at)
         VALUES (?, ?, 'repo', 'https://example.invalid/repo.git', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("repo");
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "in_progress".to_owned(),
        "history",
    )
    .await;
    let states = [
        "requested",
        "running",
        "plan_ready",
        "awaiting_approval",
        "materialized",
        "failed",
        "cancelled",
        "uncertain",
    ];
    let mut ids = Vec::new();
    for state in states {
        let id = new_uuid_v4();
        sqlx::query(
            "INSERT INTO task_coordination_request (
                id, task_id, authorization_version, mode, brief_json, objective_digest,
                request_digest, idempotency_key, state, version, created_at, updated_at
             ) VALUES (?, ?, 1, 'prepare_and_execute', '{}', ?, ?, ?, ?, 7, ?, ?)",
        )
        .bind(&id)
        .bind(&task_id)
        .bind(digest64("objective"))
        .bind(digest64("request"))
        .bind(format!("historical-{state}"))
        .bind(state)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical request");
        ids.push((id, state));
    }
    let before = schema_names(&pool).await;
    let before_rows: Vec<(String, String, i64, String, String)> = sqlx::query_as(
        "SELECT id, task_id, version, state, kind FROM task_coordination_request ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("rows before");

    run_migrations(&pool).await.expect("V171-V174 upgrade");
    run_migrations(&pool).await.expect("idempotent upgrade");

    // Every previous trigger/index/view/table name survives the single rebuild.
    let after = schema_names(&pool).await;
    assert!(
        before.is_subset(&after),
        "no schema object is lost: {:?}",
        before.difference(&after).collect::<Vec<_>>()
    );
    let after_rows: Vec<(String, String, i64, String, String)> = sqlx::query_as(
        "SELECT id, task_id, version, state, kind FROM task_coordination_request ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("rows after");
    assert_eq!(before_rows, after_rows, "complete history is preserved");
    for (id, _state) in &ids {
        let (kind, subject, resumes): (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT kind, source_subject_kind, resumes_request_id
             FROM task_coordination_request WHERE id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .expect("preserved request");
        assert_eq!(kind, "work_unit_plan");
        assert!(subject.is_none() && resumes.is_none());
        assert!(
            sqlx::query("UPDATE task_coordination_request SET brief_json = '{} ' WHERE id = ?")
                .bind(id)
                .execute(&pool)
                .await
                .is_err(),
            "identity guard survives the rebuild"
        );
        assert!(
            sqlx::query(
                "UPDATE task_coordination_request SET source_subject_id = 'x' WHERE id = ?"
            )
            .bind(id)
            .execute(&pool)
            .await
            .is_err(),
            "new source columns are immutable too"
        );
    }
    let stale_references: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_master
         WHERE sql LIKE '%task_coordination_request_v1%'
            OR sql LIKE '%project_plan_batch_coordination_request_v1%'",
    )
    .fetch_one(&pool)
    .await
    .expect("reference audit");
    assert_eq!(stale_references, 0, "no dependent object is retargeted");
    let violations: i64 = sqlx::query_scalar("SELECT count(*) FROM pragma_foreign_key_check")
        .fetch_one(&pool)
        .await
        .expect("foreign key audit");
    assert_eq!(violations, 0);
    let fk_enabled: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(&pool)
        .await
        .expect("foreign keys enabled");
    assert_eq!(fk_enabled, 1);
}

#[tokio::test]
async fn v172_request_kinds_and_batch_origins_are_exact() {
    let f = fixture(serde_json::json!([])).await;
    let run_id = new_uuid_v4();
    let digest = digest64("subject");
    let other_digest = digest64("other");

    // Structural CHECKs per kind.
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "bogus",
            None,
            None
        )
        .await
        .is_err(),
        "unknown kind"
    );
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "validation_findings",
            None,
            None
        )
        .await
        .is_err(),
        "validation_findings needs its exact subject"
    );
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "work_unit_plan",
            Some(("validation_run", run_id.as_str(), digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "a plan request has no source subject"
    );
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "validation_findings",
            Some(("external_feedback_set", run_id.as_str(), digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "wrong subject kind for the request kind"
    );
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "human_decision_resume",
            Some(("human_decision", run_id.as_str(), digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "human_decision_resume needs resumes_request_id"
    );

    // validation_findings: source must be a failed/error/stale run (or regression).
    let definition = publish(&f, "svc", "cargo test").await;
    set_pins(
        &f,
        serde_json::json!([pin("svc", &definition.definition_digest, "on_request")]),
    )
    .await;
    let request = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        check_request_input(&f, &definition, "k1", false),
        None,
    )
    .await
    .expect("queue")
    .request;
    let running = ValidationCheckRequestRepo::transition_validation_check_request(
        &f.db,
        transition(&request, ValidationCheckRequestState::Running),
        None,
    )
    .await
    .expect("running")
    .request;
    let candidate = run_catalog_check(&f, &running, ValidationRunStatus::Passed, 0).await;
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "validation_findings",
            Some(("validation_run", candidate.id.as_str(), digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "a passed run is not a findings source"
    );
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "validation_findings",
            Some(("validation_run", "missing-run", digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "source must exist"
    );

    // A failed run is a valid source; one exact request per exact source.
    let request2 = ValidationCheckRequestRepo::create_validation_check_request(
        &f.db,
        {
            let mut input = check_request_input(&f, &definition, "k2", false);
            input.commit_sha = "f".repeat(40);
            input
        },
        None,
    )
    .await
    .expect("queue 2")
    .request;
    let running2 = ValidationCheckRequestRepo::transition_validation_check_request(
        &f.db,
        transition(&request2, ValidationCheckRequestState::Running),
        None,
    )
    .await
    .expect("running 2")
    .request;
    let failed = run_catalog_check(&f, &running2, ValidationRunStatus::Failed, 1).await;
    let findings_id = new_uuid_v4();
    insert_request(
        &f.db,
        &f.task_id,
        &findings_id,
        "requested",
        "validation_findings",
        Some(("validation_run", failed.id.as_str(), digest.as_str())),
        None,
    )
    .await
    .expect("failed run is a findings source");
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "validation_findings",
            Some(("validation_run", failed.id.as_str(), digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "exactly one request per exact source"
    );
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "running",
            "validation_findings",
            Some(("validation_run", failed.id.as_str(), other_digest.as_str())),
            None,
        )
        .await
        .is_err(),
        "new kinds are born requested"
    );

    // The request is typed for the repo reader.
    let stored = crate::TaskCoordinationRepo::get_task_coordination_request(&f.db, &findings_id)
        .await
        .expect("request read")
        .expect("request exists");
    assert_eq!(
        stored.kind,
        crate::TaskCoordinationRequestKind::ValidationFindings
    );
    assert_eq!(
        stored.source_subject_kind.as_deref(),
        Some("validation_run")
    );
    assert_eq!(
        stored.source_subject_id.as_deref(),
        Some(failed.id.as_str())
    );
    assert_eq!(
        stored.source_subject_digest.as_deref(),
        Some(digest.as_str())
    );
    assert!(stored.resumes_request_id.is_none());
    assert!(stored.review_report_artifact_id.is_none());

    // Triage is the only way to reach `triaged`, and it is immutable evidence.
    sqlx::query(
        "UPDATE task_coordination_request SET state = 'running', execution_id = ?, version = 2
         WHERE id = ?",
    )
    .bind(&f.execution_id)
    .bind(&findings_id)
    .execute(f.db.pool())
    .await
    .expect("request starts");
    assert!(
        sqlx::query(
            "UPDATE task_coordination_request SET state = 'triaged', version = 3 WHERE id = ?"
        )
        .bind(&findings_id)
        .execute(f.db.pool())
        .await
        .is_err(),
        "triaged without a decision"
    );
    let triage = TaskValidationTriage {
        request_id: findings_id.clone(),
        task_id: f.task_id.clone(),
        validation_run_id: failed.id.clone(),
        comparison_id: None,
        run_digest: digest.clone(),
        execution_id: f.execution_id.clone(),
        authorization_version: 1,
        disposition: ValidationTriageDisposition::Correct,
        rationale: "regression in touched code".to_owned(),
        instructions: "fix the failing assertion".to_owned(),
        decision_digest: digest64("decision"),
        review_work_unit_id: None,
        rework_ref: None,
        created_at: now_rfc3339(),
    };
    let mut wrong_digest = triage.clone();
    wrong_digest.run_digest = digest64("different");
    assert!(
        TaskValidationTriageRepo::insert_task_validation_triage(&f.db, wrong_digest)
            .await
            .is_err(),
        "triage must bind the request's exact source digest"
    );
    let stored_triage =
        TaskValidationTriageRepo::insert_task_validation_triage(&f.db, triage.clone())
            .await
            .expect("exact triage");
    assert_eq!(
        stored_triage.disposition,
        ValidationTriageDisposition::Correct
    );
    let replay = TaskValidationTriageRepo::insert_task_validation_triage(&f.db, triage.clone())
        .await
        .expect("identical replay");
    assert_eq!(replay.decision_digest, triage.decision_digest);
    let mut changed = triage.clone();
    changed.decision_digest = digest64("another decision");
    assert!(matches!(
        TaskValidationTriageRepo::insert_task_validation_triage(&f.db, changed).await,
        Err(DbError::IdempotencyConflict)
    ));
    for statement in [
        "UPDATE task_validation_triage SET rationale = 'changed'",
        "DELETE FROM task_validation_triage",
    ] {
        assert!(sqlx::query(statement).execute(f.db.pool()).await.is_err());
    }
    sqlx::query("UPDATE task_coordination_request SET state = 'triaged', version = 3 WHERE id = ?")
        .bind(&findings_id)
        .execute(f.db.pool())
        .await
        .expect("decision recorded: request may be triaged");
    assert!(
        TaskValidationTriageRepo::get_task_validation_triage_for_run(&f.db, &failed.id, 1)
            .await
            .expect("lookup")
            .is_some()
    );

    // Batch origin CHECK: shape per origin (scope guards need a running batch, so
    // the foreign-key-free shape is verified on a disposable table copy).
    let (kind_ok,): (i64,) = sqlx::query_as(
        "SELECT count(*) FROM sqlite_master
         WHERE name = 'project_plan_batch_coordination_request'
           AND sql LIKE '%coordinator_replan%' AND sql LIKE '%validation_triage%'
           AND sql LIKE '%external_feedback_triage%' AND sql LIKE '%human_decision_resume%'",
    )
    .fetch_one(f.db.pool())
    .await
    .expect("batch origin schema");
    assert_eq!(kind_ok, 1);
    let _ = (&f.repo_id, &f.request_id);
}

#[tokio::test]
async fn v172_coordinator_replans_are_limited_to_two_and_forward_only() {
    let f = fixture(serde_json::json!([])).await;
    let new_replan = |reason: &str| CreateCoordinatorReplan {
        id: new_uuid_v4(),
        task_id: f.task_id.clone(),
        authorization_version: 1,
        source_request_id: f.request_id.clone(),
        producer_execution_id: f.execution_id.clone(),
        reason: reason.to_owned(),
        evidence_links_json: "[]".to_owned(),
        created_at: now_rfc3339(),
    };
    let first =
        TaskCoordinatorReplanRepo::create_coordinator_replan(&f.db, new_replan("split task"))
            .await
            .expect("first replan");
    assert_eq!(first.ordinal, 1);
    assert_eq!(first.state, CoordinatorReplanState::Proposed);
    let second =
        TaskCoordinatorReplanRepo::create_coordinator_replan(&f.db, new_replan("split again"))
            .await
            .expect("second replan");
    assert_eq!(second.ordinal, 2);
    assert_eq!(MAX_COORDINATOR_REPLANS_PER_TASK, 2);
    assert!(matches!(
        TaskCoordinatorReplanRepo::create_coordinator_replan(&f.db, new_replan("third")).await,
        Err(DbError::CoordinatorReplanLimitReached)
    ));
    // The database also refuses a third row and non-contiguous ordinals directly.
    for ordinal in [3_i64, 1_i64] {
        let rejected = sqlx::query(
            "INSERT INTO task_coordinator_replan (
                id, task_id, ordinal, authorization_version, source_request_id,
                producer_execution_id, reason, evidence_links_json, state, version,
                created_at, updated_at
             ) VALUES (?, ?, ?, 1, ?, ?, 'x', '[]', 'proposed', 1, ?, ?)",
        )
        .bind(new_uuid_v4())
        .bind(&f.task_id)
        .bind(ordinal)
        .bind(&f.request_id)
        .bind(&f.execution_id)
        .bind(now_rfc3339())
        .bind(now_rfc3339())
        .execute(f.db.pool())
        .await;
        assert!(rejected.is_err(), "ordinal {ordinal}");
    }
    assert_eq!(
        TaskCoordinatorReplanRepo::list_coordinator_replans_by_task(&f.db, &f.task_id)
            .await
            .expect("list")
            .len(),
        2
    );

    let step = |replan: &crate::TaskCoordinatorReplan, to_state| TransitionCoordinatorReplan {
        id: replan.id.clone(),
        expected_version: replan.version,
        to_state,
        plan_request_id: None,
        classification: None,
        classification_facts_json: None,
        human_decision_id: None,
        updated_at: now_rfc3339(),
    };
    let planning = TaskCoordinatorReplanRepo::transition_coordinator_replan(
        &f.db,
        step(&first, CoordinatorReplanState::Planning),
    )
    .await
    .expect("proposed -> planning");
    assert_eq!(planning.version, 2);
    let mut stale = step(&first, CoordinatorReplanState::Failed);
    stale.expected_version = 1;
    assert!(matches!(
        TaskCoordinatorReplanRepo::transition_coordinator_replan(&f.db, stale).await,
        Err(DbError::VersionConflict)
    ));
    // Backwards transitions and identity rewrites are rejected.
    assert!(TaskCoordinatorReplanRepo::transition_coordinator_replan(
        &f.db,
        step(&planning, CoordinatorReplanState::Proposed),
    )
    .await
    .is_err());
    assert!(
        sqlx::query("UPDATE task_coordinator_replan SET reason = 'rewritten', state = 'failed', version = 3 WHERE id = ?")
            .bind(&planning.id)
            .execute(f.db.pool())
            .await
            .is_err()
    );
    let mut classified = step(&planning, CoordinatorReplanState::AwaitingHuman);
    classified.classification =
        Some(crate::CoordinatorReplanClassification::ScopeOrAuthorityChange);
    classified.classification_facts_json =
        Some(r#"{"changed":["allow_new_task_roles"]}"#.to_owned());
    let awaiting = TaskCoordinatorReplanRepo::transition_coordinator_replan(&f.db, classified)
        .await
        .expect("planning -> awaiting_human");
    assert_eq!(
        awaiting.classification,
        Some(crate::CoordinatorReplanClassification::ScopeOrAuthorityChange)
    );
    let rejected = TaskCoordinatorReplanRepo::transition_coordinator_replan(
        &f.db,
        step(&awaiting, CoordinatorReplanState::Rejected),
    )
    .await
    .expect("awaiting_human -> rejected");
    assert!(
        TaskCoordinatorReplanRepo::transition_coordinator_replan(
            &f.db,
            step(&rejected, CoordinatorReplanState::Approved),
        )
        .await
        .is_err(),
        "terminal"
    );
    assert!(
        sqlx::query("DELETE FROM task_coordinator_replan WHERE id = ?")
            .bind(&second.id)
            .execute(f.db.pool())
            .await
            .is_err(),
        "hard-limit evidence cannot be deleted"
    );
}

fn decision_options() -> String {
    serde_json::json!([
        {"id": "waive", "label": "Waive", "description": "Accept the failure.",
         "consequence": "Gate may continue.", "authority_effect": "scope"},
        {"id": "fix", "label": "Fix first", "description": "Open rework.",
         "consequence": "One more cycle.", "authority_effect": "none"},
    ])
    .to_string()
}

fn decision_input(f: &Fixture, source_id: &str) -> CreateHumanDecisionRequest {
    CreateHumanDecisionRequest {
        id: new_uuid_v4(),
        task_id: f.task_id.clone(),
        source_kind: HumanDecisionSourceKind::CoordinationRequest,
        source_id: source_id.to_owned(),
        raised_by_execution_id: Some(f.execution_id.clone()),
        authorization_version: Some(1),
        question: "Waive the pre-existing failure?".to_owned(),
        options_json: decision_options(),
        recommended_option_id: Some("fix".to_owned()),
        recommendation_rationale: Some("The code is touched by this Task.".to_owned()),
        evidence_links_json: serde_json::json!([
            {"kind": "validation_run", "id": "run-1", "digest": digest64("run")},
            {"kind": "execution", "id": "exec-1", "digest": null}
        ])
        .to_string(),
        max_authority_effect: AuthorityEffect::Scope,
        resume_binding_json: serde_json::json!({
            "request_id": f.request_id,
            "request_kind": "work_unit_plan",
            "authorization_version": 1,
            "source_subject": {"kind": "validation_run", "id": "run-1", "digest": digest64("run")},
        })
        .to_string(),
        created_at: now_rfc3339(),
    }
}

#[tokio::test]
async fn v174_human_decisions_validate_options_evidence_and_binding() {
    let f = fixture(serde_json::json!([])).await;
    let input = decision_input(&f, &f.request_id);
    let created = HumanDecisionRepo::create_human_decision_request(
        &f.db,
        input.clone(),
        Some(v17x_event(
            &f.task_id,
            "task.human_decision_requested",
            &input.id,
        )),
    )
    .await
    .expect("decision opens");
    assert!(created.created);
    assert!(created.event.is_some());
    assert_eq!(created.decision.state, HumanDecisionState::Open);
    assert_eq!(created.decision.version, 1);
    assert_eq!(
        created.decision.max_authority_effect,
        AuthorityEffect::Scope
    );

    // One question per exact source: replay returns the row, a different question conflicts.
    let mut replay = input.clone();
    replay.id = new_uuid_v4();
    let replayed = HumanDecisionRepo::create_human_decision_request(&f.db, replay.clone(), None)
        .await
        .expect("replay");
    assert!(!replayed.created);
    assert_eq!(replayed.decision.id, input.id);
    replay.question = "Something else?".to_owned();
    assert!(matches!(
        HumanDecisionRepo::create_human_decision_request(&f.db, replay, None).await,
        Err(DbError::IdempotencyConflict)
    ));

    let mutated = |source: &str, mutate: &dyn Fn(&mut CreateHumanDecisionRequest)| {
        let mut next = decision_input(&f, source);
        mutate(&mut next);
        next
    };
    let bad_inputs: Vec<(&str, CreateHumanDecisionRequest)> = vec![
        (
            "one option",
            mutated("s-1", &|d| {
                d.options_json = serde_json::json!([{"id": "a", "label": "A", "description": "d",
                "consequence": "c", "authority_effect": "none"}])
                .to_string();
                d.recommended_option_id = None;
                d.recommendation_rationale = None;
                d.max_authority_effect = AuthorityEffect::None;
            }),
        ),
        (
            "duplicate option ids",
            mutated("s-2", &|d| {
                d.options_json = serde_json::json!([
                {"id": "a", "label": "A", "description": "d", "consequence": "c", "authority_effect": "none"},
                {"id": "a", "label": "B", "description": "d", "consequence": "c", "authority_effect": "none"}
            ]).to_string();
                d.recommended_option_id = None;
                d.recommendation_rationale = None;
                d.max_authority_effect = AuthorityEffect::None;
            }),
        ),
        (
            "bad slug",
            mutated("s-3", &|d| {
                d.options_json = serde_json::json!([
                {"id": "Bad Id", "label": "A", "description": "d", "consequence": "c", "authority_effect": "none"},
                {"id": "b", "label": "B", "description": "d", "consequence": "c", "authority_effect": "none"}
            ]).to_string();
                d.recommended_option_id = None;
                d.recommendation_rationale = None;
                d.max_authority_effect = AuthorityEffect::None;
            }),
        ),
        (
            "recommendation names no option",
            mutated("s-4", &|d| {
                d.recommended_option_id = Some("nope".to_owned());
            }),
        ),
        (
            "max effect below the option maximum",
            mutated("s-5", &|d| {
                d.max_authority_effect = AuthorityEffect::None;
            }),
        ),
        (
            "max effect above the option maximum",
            mutated("s-6", &|d| {
                d.max_authority_effect = AuthorityEffect::Authority;
            }),
        ),
        (
            "unknown evidence kind",
            mutated("s-7", &|d| {
                d.evidence_links_json =
                    serde_json::json!([{"kind": "nope", "id": "x", "digest": null}]).to_string();
            }),
        ),
        (
            "short evidence digest",
            mutated("s-8", &|d| {
                d.evidence_links_json =
                    serde_json::json!([{"kind": "execution", "id": "x", "digest": "abc"}])
                        .to_string();
            }),
        ),
        (
            "binding to a request that does not exist",
            mutated("s-9", &|d| {
                d.resume_binding_json = serde_json::json!({
                    "request_id": "missing-request", "request_kind": "work_unit_plan",
                    "authorization_version": 1, "source_subject": {}
                })
                .to_string();
            }),
        ),
        (
            "binding with a stale authorization",
            mutated("s-10", &|d| {
                d.resume_binding_json = serde_json::json!({
                    "request_id": f.request_id, "request_kind": "work_unit_plan",
                    "authorization_version": 9, "source_subject": {}
                })
                .to_string();
            }),
        ),
        (
            "binding with the wrong request kind",
            mutated("s-11", &|d| {
                d.resume_binding_json = serde_json::json!({
                    "request_id": f.request_id, "request_kind": "review_findings",
                    "authorization_version": 1, "source_subject": {}
                })
                .to_string();
            }),
        ),
    ];
    for (label, bad) in bad_inputs {
        assert!(
            HumanDecisionRepo::create_human_decision_request(&f.db, bad, None)
                .await
                .is_err(),
            "{label}"
        );
    }
}

#[tokio::test]
async fn v174_human_decisions_answer_once_and_gate_the_exact_resume_request() {
    let f = fixture(serde_json::json!([])).await;
    let input = decision_input(&f, &f.request_id);
    let open = HumanDecisionRepo::create_human_decision_request(&f.db, input.clone(), None)
        .await
        .expect("open")
        .decision;
    let answer = |decision: &crate::HumanDecisionRequest, option: &str| AnswerHumanDecision {
        id: decision.id.clone(),
        expected_version: decision.version,
        option_id: option.to_owned(),
        answered_by_user_id: f.user_id.clone(),
        note: Some("approved by the owner".to_owned()),
        answered_at: now_rfc3339(),
    };

    // A resume request cannot exist before the decision is answered.
    let resume_id = new_uuid_v4();
    let source_digest = digest64("run");
    let again_digest = digest64("again");
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &resume_id,
            "requested",
            "human_decision_resume",
            Some(("human_decision", open.id.as_str(), source_digest.as_str())),
            Some(f.request_id.as_str()),
        )
        .await
        .is_err(),
        "decision still open"
    );

    assert!(
        HumanDecisionRepo::answer_human_decision(&f.db, answer(&open, "not-an-option"), None)
            .await
            .is_err(),
        "answer must name an option"
    );
    let mut stale = answer(&open, "waive");
    stale.expected_version = 5;
    assert!(matches!(
        HumanDecisionRepo::answer_human_decision(&f.db, stale, None).await,
        Err(DbError::VersionConflict)
    ));
    let answered = HumanDecisionRepo::answer_human_decision(
        &f.db,
        answer(&open, "waive"),
        Some(v17x_event(
            &f.task_id,
            "task.human_decision_answered",
            &open.id,
        )),
    )
    .await
    .expect("answer");
    assert!(answered.event.is_some());
    assert_eq!(answered.decision.state, HumanDecisionState::Answered);
    assert_eq!(answered.decision.version, 2);
    assert_eq!(
        answered.decision.answered_option_id.as_deref(),
        Some("waive")
    );
    assert_eq!(
        answered.decision.answered_by_user_id.as_deref(),
        Some(f.user_id.as_str())
    );
    // Answered exactly once; a second answer, supersede or cancel conflicts.
    assert!(matches!(
        HumanDecisionRepo::answer_human_decision(&f.db, answer(&answered.decision, "fix"), None)
            .await,
        Err(DbError::VersionConflict)
    ));
    assert!(matches!(
        HumanDecisionRepo::close_human_decision(
            &f.db,
            &open.id,
            answered.decision.version,
            HumanDecisionState::Cancelled,
            &now_rfc3339(),
            None,
        )
        .await,
        Err(DbError::VersionConflict)
    ));
    for statement in [
        "UPDATE human_decision_request SET answered_option_id = 'fix' WHERE id = ?",
        "UPDATE human_decision_request SET question = 'changed' WHERE id = ?",
        "DELETE FROM human_decision_request WHERE id = ?",
    ] {
        assert!(
            sqlx::query(statement)
                .bind(&open.id)
                .execute(f.db.pool())
                .await
                .is_err(),
            "{statement}"
        );
    }

    // The resume request must continue exactly the bound request, at its authorization.
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "human_decision_resume",
            Some(("human_decision", open.id.as_str(), source_digest.as_str())),
            Some(answered.decision.id.as_str()),
        )
        .await
        .is_err(),
        "resumes a different request than the one the question was raised against"
    );
    insert_request(
        &f.db,
        &f.task_id,
        &resume_id,
        "requested",
        "human_decision_resume",
        Some(("human_decision", open.id.as_str(), source_digest.as_str())),
        Some(f.request_id.as_str()),
    )
    .await
    .expect("exact resume request");
    // Binding the wrong request is rejected; the exact one binds once.
    assert!(HumanDecisionRepo::bind_human_decision_resume_request(
        &f.db,
        &open.id,
        answered.decision.version,
        &f.request_id,
        &now_rfc3339(),
    )
    .await
    .is_err());
    let bound = HumanDecisionRepo::bind_human_decision_resume_request(
        &f.db,
        &open.id,
        answered.decision.version,
        &resume_id,
        &now_rfc3339(),
    )
    .await
    .expect("bind resume");
    assert_eq!(bound.resume_request_id.as_deref(), Some(resume_id.as_str()));
    assert_eq!(bound.version, 3);
    assert!(
        insert_request(
            &f.db,
            &f.task_id,
            &new_uuid_v4(),
            "requested",
            "human_decision_resume",
            Some(("human_decision", open.id.as_str(), again_digest.as_str())),
            Some(f.request_id.as_str()),
        )
        .await
        .is_err(),
        "one resume per answered decision"
    );

    // Superseding an open decision frees it for the audit trail but not for answers.
    let other = HumanDecisionRepo::create_human_decision_request(
        &f.db,
        decision_input(&f, "other-source"),
        None,
    )
    .await
    .expect("second decision")
    .decision;
    let closed = HumanDecisionRepo::close_human_decision(
        &f.db,
        &other.id,
        other.version,
        HumanDecisionState::Superseded,
        &now_rfc3339(),
        None,
    )
    .await
    .expect("supersede");
    assert_eq!(closed.decision.state, HumanDecisionState::Superseded);
    assert!(HumanDecisionRepo::close_human_decision(
        &f.db,
        &other.id,
        closed.decision.version,
        HumanDecisionState::Answered,
        &now_rfc3339(),
        None,
    )
    .await
    .is_err());
    assert!(matches!(
        HumanDecisionRepo::answer_human_decision(&f.db, answer(&closed.decision, "waive"), None)
            .await,
        Err(DbError::VersionConflict)
    ));
    let open_only = HumanDecisionRepo::list_human_decision_requests_by_task(
        &f.db,
        &f.task_id,
        Some(HumanDecisionState::Open),
        &page(10),
    )
    .await
    .expect("list open");
    assert!(open_only.items.is_empty());
    let found = HumanDecisionRepo::get_human_decision_request_by_source(
        &f.db,
        HumanDecisionSourceKind::CoordinationRequest,
        &f.request_id,
        Some(1),
    )
    .await
    .expect("by source")
    .expect("decision found");
    assert_eq!(found.id, open.id);
}

#[tokio::test]
async fn v174_migration_adds_phase_one_storage_additively() {
    let (pool, db, _migrations) = legacy_baseline_db(170).await;
    let before = schema_names(&pool).await;
    run_migrations(&pool).await.expect("V171-V174 upgrade");
    let after = schema_names(&pool).await;
    assert!(before.is_subset(&after));
    for table in [
        "validation_check_definition",
        "validation_check_request",
        "validation_comparison",
        "task_validation_triage",
        "task_coordinator_replan",
        "human_decision_request",
    ] {
        assert!(
            after.contains(&("table".to_owned(), table.to_owned())),
            "{table} exists"
        );
    }
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _migration WHERE version >= 171 ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("receipts");
    assert!(versions.contains(&172));
    let _ = db;
}
