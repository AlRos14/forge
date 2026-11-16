//! Phase 3 receipt persistence (A5): immutable fallback receipts and the one
//! allowed pending-to-terminal micro-fix bounds advance.

use super::{seed_agent, seed_project, seed_task, seed_user, sqlite_db};
use crate::{
    new_uuid_v4, now_rfc3339, ActorKind, AdvanceMicroFixReceiptBounds, CostClass,
    CreateMicroFixReceipt, CreateRoleMembership, CreateTaskRole, CreateWorkUnitFallbackReceipt,
    DbError, MicroFixBoundsOutcome, MicroFixBoundsViolation, MicroFixReceiptRepo,
    MicroFixSourceKind, RoleMembershipRepo, SqliteDb, TaskRoleRepo, WorkUnitFallbackReceiptRepo,
};
use sha2::Digest as _;

fn digest(seed: &str) -> String {
    hex::encode(sha2::Sha256::digest(seed.as_bytes()))
}

struct Fixture {
    db: SqliteDb,
    task_id: String,
    role_id: String,
    work_unit_id: String,
    origin_execution_id: String,
    origin_agent_id: String,
    fallback_agent_id: String,
    envelope_digest: String,
    request_id: String,
}

async fn fixture() -> Fixture {
    let db = sqlite_db().await;
    let task_user_id = seed_user(&db).await;
    let project_id = seed_project(&db, "v180 receipts", Some(task_user_id.clone())).await;
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
    .expect("repo creates");
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "in_progress".to_owned(),
        "v180 receipt task",
    )
    .await;

    let origin_agent_id = seed_agent(&db, "origin", "account", Some(task_user_id.clone())).await;
    let fallback_agent_id =
        seed_agent(&db, "fallback", "account", Some(task_user_id.clone())).await;
    let coordinator_agent_id =
        seed_agent(&db, "coordinator", "account", Some(task_user_id.clone())).await;

    let role_id = new_uuid_v4();
    TaskRoleRepo::create(
        &db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: task_id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("implementer TaskRole creates");
    for agent_id in [&origin_agent_id, &fallback_agent_id] {
        RoleMembershipRepo::add(
            &db,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: role_id.clone(),
                actor_kind: ActorKind::Agent,
                actor_id: agent_id.clone(),
                status: crate::RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("implementer membership creates");
    }
    let coordinator_role_id = new_uuid_v4();
    TaskRoleRepo::create(
        &db,
        CreateTaskRole {
            id: coordinator_role_id.clone(),
            task_id: task_id.clone(),
            role: "coordinator".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("coordinator TaskRole creates");
    RoleMembershipRepo::add(
        &db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: coordinator_role_id.clone(),
            actor_kind: ActorKind::Agent,
            actor_id: coordinator_agent_id.clone(),
            status: crate::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("coordinator membership creates");

    let work_unit_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO work_unit (
            id, task_id, title, scope, status, role, assigned_actor_kind,
            assigned_actor_id, requires_integration, created_by_kind, created_by_id,
            version, created_at, updated_at
         ) VALUES (?, ?, 'implement', 'src/**', 'open', 'implementer', 'agent', ?,
                   0, 'human', ?, 1, ?, ?)",
    )
    .bind(&work_unit_id)
    .bind(&task_id)
    .bind(&origin_agent_id)
    .bind(&task_user_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("WorkUnit creates");

    let origin_execution_id = new_uuid_v4();
    // This fixture isolates receipt guards from normal provider admission. The
    // WorkUnit and its exact TaskRole snapshot remain real and FK-backed.
    sqlx::query("DROP TRIGGER work_unit_execution_admission_guard")
        .execute(db.pool())
        .await
        .expect("disable only the provider-admission fixture guard");
    sqlx::query(
        "INSERT INTO execution (
            id, task_id, agent_id, actor_kind, actor_id, role, status,
            work_unit_id, work_unit_version, created_at, updated_at
         ) VALUES (?, ?, ?, 'agent', ?, 'implementer', 'failed', ?, 1, ?, ?)",
    )
    .bind(&origin_execution_id)
    .bind(&task_id)
    .bind(&origin_agent_id)
    .bind(&origin_agent_id)
    .bind(&work_unit_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("failed source Execution creates");
    sqlx::query(
        "UPDATE work_unit SET assigned_actor_id = ?, version = 2, updated_at = ? WHERE id = ?",
    )
    .bind(&fallback_agent_id)
    .bind(&now)
    .bind(&work_unit_id)
    .execute(db.pool())
    .await
    .expect("WorkUnit reallocates to the authorized fallback Agent");

    let envelope = serde_json::json!({
        "schema_version": 2,
        "fallback_agents": [{
            "task_role_id": role_id,
            "agent_id": fallback_agent_id,
            "cost_class": "economy",
            "max_wait_seconds": 30
        }],
        "micro_fix": {
            "enabled": true,
            "max_files": 3,
            "max_changed_lines": 60,
            "max_per_task": 3,
            "extra_forbidden_globs": []
        }
    });
    let envelope_digest = digest(&envelope.to_string());
    sqlx::query(
        "UPDATE task_coordination SET mode = 'prepare_and_execute',
            coordinator_task_role_id = ?, coordinator_actor_kind = 'agent',
            coordinator_actor_id = ?, authorization_version = 1,
            envelope_json = ?, envelope_digest = ?, requested_by = ?, team_stale = 0
         WHERE task_id = ?",
    )
    .bind(&coordinator_role_id)
    .bind(&coordinator_agent_id)
    .bind(envelope.to_string())
    .bind(&envelope_digest)
    .bind(&task_user_id)
    .bind(&task_id)
    .execute(db.pool())
    .await
    .expect("current coordination authorization is installed");

    let request_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json, objective_digest,
            request_digest, idempotency_key, state, version, created_at, updated_at
         ) VALUES (?, ?, 1, 'prepare_and_execute', '{}', ?, ?, ?, 'running', 1, ?, ?)",
    )
    .bind(&request_id)
    .bind(&task_id)
    .bind(digest("objective"))
    .bind(digest("request"))
    .bind(format!("v180-{request_id}"))
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("coordination request creates");

    Fixture {
        db,
        task_id,
        role_id,
        work_unit_id,
        origin_execution_id,
        origin_agent_id,
        fallback_agent_id,
        envelope_digest,
        request_id,
    }
}

fn fallback_input(f: &Fixture, id: String) -> CreateWorkUnitFallbackReceipt {
    CreateWorkUnitFallbackReceipt {
        id,
        task_id: f.task_id.clone(),
        work_unit_id: f.work_unit_id.clone(),
        work_unit_version_before: 1,
        work_unit_version_after: 2,
        origin_execution_id: f.origin_execution_id.clone(),
        failure_class: "executor_unavailable".to_owned(),
        route_attempts_digest: digest("route attempts"),
        task_role_id: f.role_id.clone(),
        from_actor_kind: ActorKind::Agent,
        from_actor_id: f.origin_agent_id.clone(),
        to_actor_kind: ActorKind::Agent,
        to_actor_id: f.fallback_agent_id.clone(),
        chain_index: 0,
        cost_class: CostClass::Economy,
        authorization_version: 1,
        envelope_digest: f.envelope_digest.clone(),
        new_execution_id: None,
        created_at: now_rfc3339(),
    }
}

#[tokio::test]
async fn v180_fallback_receipt_is_immutable_and_unique_per_origin_execution() {
    let f = fixture().await;
    let receipt = WorkUnitFallbackReceiptRepo::create_work_unit_fallback_receipt(
        &f.db,
        fallback_input(&f, new_uuid_v4()),
    )
    .await
    .expect("authorized fallback receipt inserts");
    assert_eq!(receipt.origin_execution_id, f.origin_execution_id);
    assert_eq!(receipt.to_actor_id, f.fallback_agent_id);

    assert!(
        sqlx::query("UPDATE work_unit_fallback_receipt SET cost_class = 'premium' WHERE id = ?")
            .bind(&receipt.id)
            .execute(f.db.pool())
            .await
            .is_err(),
        "fallback receipts do not change after insertion"
    );
    assert!(
        sqlx::query("DELETE FROM work_unit_fallback_receipt WHERE id = ?")
            .bind(&receipt.id)
            .execute(f.db.pool())
            .await
            .is_err()
    );

    assert!(
        WorkUnitFallbackReceiptRepo::create_work_unit_fallback_receipt(
            &f.db,
            fallback_input(&f, new_uuid_v4()),
        )
        .await
        .is_err()
    );
    assert_eq!(
        WorkUnitFallbackReceiptRepo::get_work_unit_fallback_receipt_by_origin_execution(
            &f.db,
            &f.origin_execution_id
        )
        .await
        .expect("origin lookup")
        .expect("receipt exists")
        .id,
        receipt.id
    );
}

#[tokio::test]
async fn v180_micro_fix_receipt_allows_one_bounds_advance_only() {
    let f = fixture().await;
    // Keep this test focused on append-only and transition guards; source
    // provenance and authorization inserts are covered by the insert guard.
    sqlx::query("DROP TRIGGER micro_fix_receipt_insert_guard")
        .execute(f.db.pool())
        .await
        .expect("disable only the source fixture guard");
    let receipt = MicroFixReceiptRepo::create_micro_fix_receipt(
        &f.db,
        CreateMicroFixReceipt {
            id: new_uuid_v4(),
            task_id: f.task_id.clone(),
            request_id: f.request_id.clone(),
            source_kind: MicroFixSourceKind::ReviewFindings,
            finding_refs_json: r#"["finding-1"]"#.to_owned(),
            declared_paths_json: r#"["src/a.rs"]"#.to_owned(),
            fix_work_unit_id: f.work_unit_id.clone(),
            reviewed_commit_sha: "a".repeat(40),
            reviewer_cost_class: None,
            reviewer_actor_kind: None,
            reviewer_actor_id: None,
            envelope_digest: f.envelope_digest.clone(),
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("pending micro-fix receipt creates");
    assert_eq!(receipt.ordinal, 1);
    assert_eq!(receipt.bounds_outcome, MicroFixBoundsOutcome::Pending);

    assert!(
        sqlx::query("UPDATE micro_fix_receipt SET reviewed_commit_sha = ? WHERE id = ?")
            .bind("b".repeat(40))
            .bind(&receipt.id)
            .execute(f.db.pool())
            .await
            .is_err(),
        "receipt identity remains immutable"
    );
    assert!(sqlx::query("DELETE FROM micro_fix_receipt WHERE id = ?")
        .bind(&receipt.id)
        .execute(f.db.pool())
        .await
        .is_err());

    let advanced = MicroFixReceiptRepo::advance_micro_fix_receipt_bounds(
        &f.db,
        AdvanceMicroFixReceiptBounds {
            id: receipt.id.clone(),
            result_commit_sha: "c".repeat(40),
            changed_files: 1,
            changed_lines: 12,
            bounds_outcome: MicroFixBoundsOutcome::WithinBounds,
            bounds_violation: None,
            updated_at: "9999-12-31T23:59:59.999Z".to_owned(),
        },
    )
    .await
    .expect("pending bounds advance once");
    assert_eq!(advanced.bounds_outcome, MicroFixBoundsOutcome::WithinBounds);

    let second_advance = MicroFixReceiptRepo::advance_micro_fix_receipt_bounds(
        &f.db,
        AdvanceMicroFixReceiptBounds {
            id: receipt.id,
            result_commit_sha: "d".repeat(40),
            changed_files: 4,
            changed_lines: 61,
            bounds_outcome: MicroFixBoundsOutcome::BoundsExceeded,
            bounds_violation: Some(MicroFixBoundsViolation::TooManyFiles),
            updated_at: "9999-12-31T23:59:59.999Z".to_owned(),
        },
    )
    .await;
    assert!(matches!(second_advance, Err(DbError::VersionConflict)));
}
