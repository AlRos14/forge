use crate::task_integration_operation::TaskIntegrationOperationManager;
use db::{ProjectRepo, SqliteDb};
use std::{path::Path, sync::Arc};

/// Reconcile stale integration-operation owners before the database's
/// fail-closed Project teardown guard checks for active operations.
/// Direct repository deletion deliberately retains that guard.
pub async fn delete_project(
    db: Arc<SqliteDb>,
    workspace_root: &Path,
    project_id: &str,
) -> crate::Result<()> {
    let task_ids =
        sqlx::query_scalar::<_, String>("SELECT id FROM task WHERE project_id = ? ORDER BY id")
            .bind(project_id)
            .fetch_all(db.pool())
            .await?;
    let operations =
        TaskIntegrationOperationManager::new(Arc::clone(&db), workspace_root.to_path_buf());
    for task_id in task_ids {
        operations.reconcile_stale(&task_id).await?;
    }
    ProjectRepo::delete(&*db, project_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        create_sqlite_pool, now_rfc3339, run_migrations, CreateDomainEvent, CreateProject,
        CreateTask, DomainEventRepo, ProjectRepo, TaskIntegrationOperationKind,
        TaskIntegrationOperationRepo, TaskRepo,
    };
    use tempfile::TempDir;

    #[tokio::test]
    async fn project_deletion_reconciles_stale_task_integration_operations() {
        let temp = TempDir::new().expect("temporary directory");
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(SqliteDb::new(pool));
        let now = now_rfc3339();
        let project_id = db::new_uuid_v4();
        let task_id = db::new_uuid_v4();
        ProjectRepo::create(
            &*db,
            CreateProject {
                id: project_id.clone(),
                name: "Stale operation teardown".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project");
        TaskRepo::create(
            &*db,
            CreateTask {
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: None,
                parent_task_id: None,
                subtask_order: None,
                assignee_type: None,
                assignee_id: None,
                title: "Stale operation".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task");
        let source_event = DomainEventRepo::append_event(
            &*db,
            CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "execution.failed".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: db::new_uuid_v4(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: "project-delete-retry-source".to_owned(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some("project-delete-retry-source".to_owned()),
                payload_json: "{}".to_owned(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("retry source event");
        let receipt_event = DomainEventRepo::append_event(
            &*db,
            CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "task.rework_requested".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: "project-delete-retry-receipt".to_owned(),
                causation_id: Some(source_event.id.clone()),
                causation_depth: 1,
                dedupe_key: Some("project-delete-retry-receipt".to_owned()),
                payload_json: "{}".to_owned(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("retry receipt event");
        sqlx::query(
            "INSERT INTO task_failure_retry_receipt (
                id, task_id, failure_kind, failure_ref, source_event_id,
                attempt_number, retry_budget, disposition, policy_ref,
                policy_version, policy_digest, receipt_event_id, created_at
             ) VALUES (?, ?, 'execution_failed', ?, ?, 1, 3, 'rework',
                       'forge.task_failure_retry', 1, ?, ?, ?)",
        )
        .bind(db::new_uuid_v4())
        .bind(&task_id)
        .bind(db::new_uuid_v4())
        .bind(&source_event.id)
        .bind("a".repeat(64))
        .bind(&receipt_event.id)
        .bind(now_rfc3339())
        .execute(db.pool())
        .await
        .expect("durable retry receipt");
        let active = TaskIntegrationOperationRepo::begin(
            &*db,
            db::CreateTaskIntegrationOperation {
                id: db::new_uuid_v4(),
                task_id: task_id.clone(),
                kind: TaskIntegrationOperationKind::IntegrationWorkspaceCleanup,
                owner_id: "crashed-project-owner".to_owned(),
                gate_evaluation_id: None,
                remote_waiting: false,
                parent_operation_id: None,
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("stale operation row");

        assert!(ProjectRepo::delete(&*db, &project_id).await.is_err());
        assert_eq!(
            TaskIntegrationOperationRepo::get_active_for_task(&*db, &task_id)
                .await
                .expect("stale operation remains after guarded direct delete")
                .expect("running row remains")
                .id,
            active.id
        );
        delete_project(Arc::clone(&db), temp.path(), &project_id)
            .await
            .expect("service reconciles then deletes the Project");
        assert!(ProjectRepo::get_by_id(&*db, &project_id)
            .await
            .expect("project lookup")
            .is_none());
    }
}
