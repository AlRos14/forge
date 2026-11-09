use super::*;
use crate::WorkspaceExecutionLockManager;
use db::{
    new_uuid_v4, now_rfc3339, ActorKind, CreateRoleMembership, CreateTaskRole, CreateWorkUnit,
    RoleMembershipRepo, TaskLifecycleState, TaskRoleRepo, UserRepo, WorkUnitProvenance,
    WorkUnitRepo, WorkUnitStatus,
};
use sha2::{Digest, Sha256};
use std::sync::Arc;

#[tokio::test]
async fn auto_completion_revalidates_coordination_inside_the_work_unit_transaction() {
    let db = Arc::new(helpers::sqlite_db().await);
    let (project_id, repo_id, _repo_dir) = helpers::seed_project_repo(&db).await;
    let task =
        helpers::seed_task_with_status(&db, &project_id, &repo_id, TaskLifecycleState::Ready).await;
    let now = now_rfc3339();
    let owner_id = new_uuid_v4();
    UserRepo::create_user(
        &*db,
        &db::User {
            id: owner_id.clone(),
            email: format!("{owner_id}@example.invalid"),
            password_hash: "unused".to_owned(),
            display_name: Some("Owner".to_owned()),
            is_admin: false,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("user");
    let role_id = new_uuid_v4();
    TaskRoleRepo::create_with_configuration(
        &*db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: task.id.clone(),
            role: "implementer".to_owned(),
            coordination_mode: Some(db::CoordinationMode::Independent),
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
        r#"{"schema_version":1,"display_label":"Implementer","capabilities":[],"rework":null}"#,
    )
    .await
    .expect("role");
    RoleMembershipRepo::add(
        &*db,
        CreateRoleMembership {
            id: new_uuid_v4(),
            task_role_id: role_id.clone(),
            actor_kind: ActorKind::Human,
            actor_id: owner_id.clone(),
            status: db::RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("membership");
    let report_id = new_uuid_v4();
    let producer_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, summary, created_at, updated_at,
             actor_kind, actor_id
         ) VALUES (?, ?, NULL, 'implementer', 'completed', '{}', ?, ?, 'human', ?)",
    )
    .bind(&producer_id)
    .bind(&task.id)
    .bind(&now)
    .bind(&now)
    .bind(&owner_id)
    .execute(db.pool())
    .await
    .expect("producer execution");
    let mut artifact_tx = db.pool().begin().await.expect("artifact transaction");
    sqlx::query(
        "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
         VALUES (?, ?, ?)",
    )
    .bind(&report_id)
    .bind(&producer_id)
    .bind(&task.id)
    .execute(&mut *artifact_tx)
    .await
    .expect("producer");
    sqlx::query(
        "INSERT INTO artifact (
             id, task_id, kind, storage_kind, content, content_ref,
             metadata_json, digest, created_at
         ) VALUES (?, ?, 'summary', 'inline', '{}', NULL, '{}', ?, ?)",
    )
    .bind(&report_id)
    .bind(&task.id)
    .bind(hex::encode(Sha256::digest(b"{}")))
    .bind(&now)
    .execute(&mut *artifact_tx)
    .await
    .expect("artifact");
    artifact_tx.commit().await.expect("artifact commit");
    let unit_id = new_uuid_v4();
    WorkUnitRepo::create(
        &*db,
        CreateWorkUnit {
            id: unit_id.clone(),
            task_id: task.id.clone(),
            parent_work_unit_id: None,
            title: "Delivery".to_owned(),
            scope: "Exact result".to_owned(),
            execution_instructions: None,
            role: "implementer".to_owned(),
            assigned_actor: Some(db::ActorRef::Human(owner_id.clone())),
            requires_integration: false,
            provenance: Some(WorkUnitProvenance::Artifact(report_id.clone())),
            artifact_input_ids: Vec::new(),
            review_approval_gate_id: None,
            review_approval_line_id: None,
            created_by: db::ActorRef::Human(owner_id.clone()),
            created_at: now.clone(),
        },
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "work_unit.created".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: unit_id.clone(),
            actor_type: "human".to_owned(),
            actor_id: Some(owner_id.clone()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("race-work-unit:{unit_id}")),
            payload_json: serde_json::json!({
                "task_id": task.id,
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
    sqlx::query(
        "INSERT INTO task_quality_request (
             id, task_id, source_kind, source_id, category, target_kind,
             target_task_role_id, target_actor_kind, target_actor_id, summary,
             findings_json, resolution_condition, state, handoff_id, work_unit_id,
             budget_round, source_digest, request_digest, idempotency_key,
             created_by_kind, created_by_id, version, created_at, updated_at
         ) VALUES (?, ?, 'human', ?, 'delivery_correction', 'implementation',
                   ?, 'human', ?, 'Correct', '[]', 'Reviewed', 'work_created', NULL, ?,
                   1, ?, ?, 'race-quality', 'human', ?, 1, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(&task.id)
    .bind(&owner_id)
    .bind(&role_id)
    .bind(&owner_id)
    .bind(&unit_id)
    .bind(hex::encode(Sha256::digest(b"source")))
    .bind(hex::encode(Sha256::digest(b"request")))
    .bind(&owner_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("quality link");
    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (
             id, task_id, agent_id, role, status, created_at, updated_at,
             actor_kind, actor_id, work_unit_id, work_unit_version
         ) VALUES (?, ?, NULL, 'implementer', 'running', ?, ?, 'human', ?, ?, 1)",
    )
    .bind(&execution_id)
    .bind(&task.id)
    .bind(&now)
    .bind(&now)
    .bind(&owner_id)
    .bind(&unit_id)
    .execute(db.pool())
    .await
    .expect("running execution");
    sqlx::query(
        "UPDATE execution SET status = 'completed', after_sha = 'race-sha',
             updated_at = ? WHERE id = ?",
    )
    .bind(now_rfc3339())
    .bind(&execution_id)
    .execute(db.pool())
    .await
    .expect("completed execution");
    sqlx::query(
        "UPDATE task_coordination
         SET mode = 'proposal', authorization_version = 5,
             coordinator_task_role_id = ?,
             coordinator_actor_kind = 'human', coordinator_actor_id = ?,
             revoked_at = NULL, team_stale = 0
         WHERE task_id = ?",
    )
    .bind(&role_id)
    .bind(&owner_id)
    .bind(&task.id)
    .execute(db.pool())
    .await
    .expect("coordination authorization");

    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let service = Arc::new(
        TaskService::new(Arc::clone(&db), Arc::new(EventBus::new(32)))
            .with_workspace_root(std::env::temp_dir())
            .with_workspace_exec_locks(Arc::new(WorkspaceExecutionLockManager::new()))
            .with_repo_cache_locks(Arc::new(::workspace::RepoCacheLockManager::new()))
            .with_auto_completion_precommit_barrier(Arc::clone(&barrier)),
    );
    let completion = {
        let service = Arc::clone(&service);
        let execution_id = execution_id.clone();
        tokio::spawn(async move {
            service
                .complete_coordination_delivery_for_execution(&execution_id)
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), barrier.wait())
        .await
        .expect("completion reached precommit barrier");
    sqlx::query("UPDATE task_coordination SET revoked_at = ? WHERE task_id = ?")
        .bind(now_rfc3339())
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("revoke during window");
    tokio::time::timeout(std::time::Duration::from_secs(15), barrier.wait())
        .await
        .expect("completion resumed after revocation");
    let completed = completion
        .await
        .expect("completion task")
        .expect("completion result");
    assert!(completed.is_none());
    let unit = WorkUnitRepo::get_by_id(&*db, &unit_id)
        .await
        .expect("unit")
        .expect("unit row");
    assert_eq!(unit.status, WorkUnitStatus::Open);

    sqlx::query("UPDATE task_coordination SET revoked_at = NULL WHERE task_id = ?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("restore revocation");
    let completion = {
        let service = Arc::clone(&service);
        let execution_id = execution_id.clone();
        tokio::spawn(async move {
            service
                .complete_coordination_delivery_for_execution(&execution_id)
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), barrier.wait())
        .await
        .expect("manual completion reached precommit barrier");
    sqlx::query("UPDATE task_coordination SET mode = 'manual' WHERE task_id = ?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("manual during window");
    tokio::time::timeout(std::time::Duration::from_secs(15), barrier.wait())
        .await
        .expect("manual completion resumed");
    assert!(completion
        .await
        .expect("manual completion task")
        .expect("manual completion result")
        .is_none());
    let unit = WorkUnitRepo::get_by_id(&*db, &unit_id)
        .await
        .expect("unit")
        .expect("unit row");
    assert_eq!(unit.status, WorkUnitStatus::Open);

    sqlx::query("UPDATE task_coordination SET mode = 'proposal' WHERE task_id = ?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("restore mode");
    let completion = {
        let service = Arc::clone(&service);
        let execution_id = execution_id.clone();
        tokio::spawn(async move {
            service
                .complete_coordination_delivery_for_execution(&execution_id)
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(15), barrier.wait())
        .await
        .expect("stale completion reached precommit barrier");
    sqlx::query("UPDATE task_coordination SET team_stale = 1 WHERE task_id = ?")
        .bind(&task.id)
        .execute(db.pool())
        .await
        .expect("stale team during window");
    tokio::time::timeout(std::time::Duration::from_secs(15), barrier.wait())
        .await
        .expect("stale completion resumed");
    assert!(completion
        .await
        .expect("stale completion task")
        .expect("stale completion result")
        .is_none());
    let unit = WorkUnitRepo::get_by_id(&*db, &unit_id)
        .await
        .expect("unit")
        .expect("unit row");
    assert_eq!(unit.status, WorkUnitStatus::Open);
}
