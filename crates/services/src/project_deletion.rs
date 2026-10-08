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
        create_sqlite_pool, now_rfc3339, run_migrations, run_migrations_from, CreateDomainEvent,
        CreateProject, CreateTask, DomainEventRepo, ProjectRepo, TaskIntegrationOperationKind,
        TaskIntegrationOperationRepo, TaskRepo,
    };
    use std::{fs, path::Path};
    use tempfile::TempDir;

    fn copy_db_migrations_through(limit: i64, destination: &Path) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("../db/migrations");
        for entry in fs::read_dir(source).expect("DB migration directory") {
            let path = entry.expect("migration entry").path();
            let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };
            let Some((version, _)) = stem.split_once("__") else {
                continue;
            };
            let version = version
                .strip_prefix('V')
                .expect("migration prefix")
                .parse::<i64>()
                .expect("migration number");
            if version <= limit {
                fs::copy(&path, destination.join(path.file_name().expect("filename")))
                    .expect("migration copied");
            }
        }
    }

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

    #[tokio::test]
    async fn delete_project_preserves_pr11_history_and_clears_only_fk_references() {
        let temp = TempDir::new().expect("temporary workspace");
        let migrations = temp.path().join("migrations");
        fs::create_dir_all(&migrations).expect("migration dir");
        copy_db_migrations_through(117, &migrations);
        let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
        run_migrations_from(&pool, &migrations)
            .await
            .expect("schema through V117");
        let now = now_rfc3339();

        sqlx::query(
            "INSERT INTO user (id, email, password_hash, created_at, updated_at)
             VALUES ('delete-owner', 'delete-owner@example.test', 'fixture', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("Project owner");
        sqlx::query(
            "INSERT INTO project (id, name, settings, workflow_definition, owner_id, created_at, updated_at)
             VALUES ('delete-project', 'Historical teardown', '{}', '{}', 'delete-owner', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("Project");
        let main_chat_id: String = sqlx::query_scalar(
            "SELECT id FROM agent_chat WHERE kind = 'account_main' AND account_id = 'delete-owner'",
        )
        .fetch_one(&pool)
        .await
        .expect("account Main Chat");
        let project_chat_id: String = sqlx::query_scalar(
            "SELECT id FROM agent_chat WHERE kind = 'project' AND project_id = 'delete-project'",
        )
        .fetch_one(&pool)
        .await
        .expect("Project Chat");
        sqlx::query(
            "INSERT INTO agent_handoff
             (id, source_chat_id, target_chat_id, content, correlation_id, dedupe_key,
              created_at, updated_at)
             VALUES ('delete-handoff', ?, ?, 'historical handoff',
                     'delete-handoff-correlation', 'delete-handoff-key', ?, ?)",
        )
        .bind(&main_chat_id)
        .bind(&project_chat_id)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical handoff");
        sqlx::query(
            "INSERT INTO agent_handoff_delivery
             (handoff_id, delivery_sequence, status, created_at)
             VALUES ('delete-handoff', 1, 'delivered', ?)",
        )
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical handoff delivery receipt");
        sqlx::query(
            "INSERT INTO agent_chat_instruction_revision
             (id, chat_id, revision, body, created_by_type, created_by_id, created_at)
             VALUES ('delete-chat-instruction', ?, 1, 'historical instruction',
                     'user', 'delete-owner', ?)",
        )
        .bind(&project_chat_id)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Project Chat instruction");
        sqlx::query(
            "INSERT INTO agent_chat_message
             (id, chat_id, sequence, author_type, author_id, content, status,
              correlation_id, source_type, created_at)
             VALUES ('delete-chat-message', ?, 1, 'user', 'delete-owner',
                     'historical Project Chat transcript', 'complete',
                     'delete-chat-correlation', 'native', ?)",
        )
        .bind(&project_chat_id)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Project Chat message");
        sqlx::query(
            "INSERT INTO agent_chat_turn_job
             (id, chat_id, triggering_message_id, canonical_scope_type,
              canonical_scope_id, status, dedupe_key, correlation_id, created_at, updated_at)
             VALUES ('delete-chat-turn', ?, 'delete-chat-message', 'agent_chat', ?,
                     'queued', 'delete-chat-turn-key', 'delete-chat-correlation', ?, ?)",
        )
        .bind(&project_chat_id)
        .bind(&project_chat_id)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical queued Project Chat turn");
        sqlx::query(
            "INSERT INTO task
             (id, project_id, repo_id, title, task_type, status, created_at, updated_at)
             VALUES ('delete-task', 'delete-project', NULL, 'Historical task',
                     'implementation', 'todo', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("Task");
        sqlx::query(
            "INSERT INTO execution (id, task_id, role, status, created_at, updated_at)
             VALUES ('delete-execution', 'delete-task', 'executor', 'completed', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Execution");
        sqlx::query(
            "INSERT INTO agent_identity (id, name, owner_id, visibility, created_at, updated_at)
             VALUES ('delete-agent', 'Historical Agent', 'delete-owner', 'global', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("Agent identity");
        sqlx::query(
            "INSERT INTO agent_profile
             (id, identity_id, backend_kind, executor_type, created_at, updated_at)
             VALUES ('delete-profile', 'delete-agent', 'cli', 'test', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("Agent profile");
        sqlx::query(
            "INSERT INTO product_genesis_session
             (id, account_id, main_chat_id, prompt_revision, prompt_body, maturity,
              lifecycle, project_id, handoff_id, created_at, updated_at)
             VALUES ('delete-genesis', 'delete-owner', ?, 'v1', 'historical idea', 'mvp',
                     'handed_off', 'delete-project', 'delete-handoff', ?, ?)",
        )
        .bind(&main_chat_id)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("handed-off Product Genesis");
        sqlx::query(
            "INSERT INTO project_charter
             (id, account_id, genesis_session_id, project_id, project_mode, maturity,
              lifecycle, created_at, updated_at)
             VALUES ('delete-charter', 'delete-owner', 'delete-genesis', 'delete-project',
                     'standard', 'mvp', 'attached', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Charter");
        sqlx::query(
            "INSERT INTO project_charter_revision
             (id, charter_id, revision, lifecycle, schema_version, render_version,
              content_json, rendered_view, author_type, author_id, content_digest,
              rendered_digest, created_at)
             VALUES ('delete-charter-r1', 'delete-charter', 1, 'approved', 'test', 'test',
                     '{}', 'historical Charter', 'user', 'delete-owner', 'charter-digest',
                     'charter-render-digest', ?)",
        )
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Charter revision");
        sqlx::query(
            "INSERT INTO project_charter_approval
             (id, approval_type, charter_id, revision_id, content_digest, rendered_digest,
              expected_charter_version, approving_principal_type, approving_principal_id,
              authorization_basis, authorization_action, explicit_event,
              authorization_occurred_at, source_action, idempotency_key, created_at, updated_at)
             VALUES ('delete-charter-approval', 'project_creation', 'delete-charter',
                     'delete-charter-r1', 'charter-digest', 'charter-render-digest', 1,
                     'user', 'delete-owner', 'historical approval', 'project.create',
                     'approve historical charter', ?, 'fixture', 'delete-charter-approval-key', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Charter approval");
        sqlx::query(
            "UPDATE product_genesis_session
             SET charter_id = 'delete-charter', charter_revision_id = 'delete-charter-r1',
                 charter_approval_id = 'delete-charter-approval', charter_version = 1
             WHERE id = 'delete-genesis'",
        )
        .execute(&pool)
        .await
        .expect("Genesis Charter provenance");
        sqlx::query(
            "UPDATE project_agent_binding
             SET state = 'replaced', replacement_reason = 'fixture replaces setup row'
             WHERE project_id = 'delete-project'",
        )
        .execute(&pool)
        .await
        .expect("retire setup binding in historical fixture");
        sqlx::query(
            "INSERT INTO project_agent_binding
             (id, project_id, identity_id, profile_id, state, charter_id,
              charter_revision_id, created_at, updated_at)
             VALUES ('delete-binding', 'delete-project', 'delete-agent', 'delete-profile',
                     'active', 'delete-charter', 'delete-charter-r1', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Project Agent binding");
        sqlx::query(
            "INSERT INTO agent_commitment
             (id, owner_identity_id, scope_type, scope_id, title, status,
              correlation_id, originating_task_id, created_at, updated_at)
             VALUES ('delete-commitment', 'delete-agent', 'task', 'delete-task',
                     'historical commitment', 'open', 'delete-commitment-correlation',
                     'delete-task', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Agent commitment");
        sqlx::query(
            "INSERT INTO memory_item
             (id, task_id, execution_id, scope_type, scope_id, source_type,
              kind, title, body, created_at)
             VALUES ('delete-memory', 'delete-task', 'delete-execution', 'task',
                     'delete-task', 'fixture', 'observation', 'historical memory',
                     'body remains unchanged', ?)",
        )
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical MemoryItem");
        sqlx::query(
            "INSERT INTO project_execution_baseline
             (id, project_id, lifecycle, created_at, updated_at)
             VALUES ('delete-baseline', 'delete-project', 'active', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Execution Baseline");
        sqlx::query(
            "INSERT INTO project_milestone
             (id, project_id, milestone_sequence, milestone_key, lifecycle, created_at, updated_at)
             VALUES ('delete-milestone', 'delete-project', 1, 'M001', 'released', ?, ?)",
        )
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Milestone");
        sqlx::query(
            "INSERT INTO domain_event
             (id, event_type, entity_type, entity_id, actor_type, actor_id,
              scope_type, scope_id, correlation_id, payload_json, created_at)
             VALUES ('delete-project-event', 'project.created', 'project',
                     'delete-project', 'user', 'delete-owner', 'project',
                     'delete-project', 'delete-project-correlation', '{}', ?)",
        )
        .bind(&now)
        .execute(&pool)
        .await
        .expect("durable Project event");

        copy_db_migrations_through(119, &migrations);
        run_migrations_from(&pool, &migrations)
            .await
            .expect("apply PR11 cutover and FK repair");
        let db = Arc::new(SqliteDb::new(pool));
        let direct_handoff_delete =
            sqlx::query("DELETE FROM agent_handoff WHERE id = 'delete-handoff'")
                .execute(db.pool())
                .await;
        assert!(
            direct_handoff_delete.is_err(),
            "direct deletion of an immutable historical handoff stays blocked"
        );
        let direct_delivery_delete =
            sqlx::query("DELETE FROM agent_handoff_delivery WHERE handoff_id = 'delete-handoff'")
                .execute(db.pool())
                .await;
        assert!(
            direct_delivery_delete.is_err(),
            "direct deletion of an immutable delivery receipt stays blocked"
        );
        let direct_message_delete =
            sqlx::query("DELETE FROM agent_chat_message WHERE id = 'delete-chat-message'")
                .execute(db.pool())
                .await;
        assert!(
            direct_message_delete.is_err(),
            "direct deletion of an immutable chat message stays blocked"
        );
        let direct_instruction_delete = sqlx::query(
            "DELETE FROM agent_chat_instruction_revision WHERE id = 'delete-chat-instruction'",
        )
        .execute(db.pool())
        .await;
        assert!(
            direct_instruction_delete.is_err(),
            "direct deletion of an immutable chat instruction stays blocked"
        );
        delete_project(Arc::clone(&db), temp.path(), "delete-project")
            .await
            .expect("productive Project deletion with historical PR11 rows");

        let genesis: (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        ) = sqlx::query_as(
            "SELECT lifecycle, project_id, handoff_id, charter_id,
                    charter_revision_id, charter_approval_id, prompt_body
             FROM product_genesis_session WHERE id = 'delete-genesis'",
        )
        .fetch_one(db.pool())
        .await
        .expect("historical Genesis survives");
        assert_eq!(
            genesis,
            (
                "handed_off".to_owned(),
                None,
                None,
                None,
                None,
                None,
                "historical idea".to_owned(),
            ),
            "only foreign-key references change"
        );
        let commitment: (String, String, Option<String>) = sqlx::query_as(
            "SELECT title, status, originating_task_id
             FROM agent_commitment WHERE id = 'delete-commitment'",
        )
        .fetch_one(db.pool())
        .await
        .expect("historical commitment survives");
        assert_eq!(
            commitment,
            ("historical commitment".to_owned(), "open".to_owned(), None)
        );
        let memory: (String, Option<String>, Option<String>) = sqlx::query_as(
            "SELECT body, task_id, execution_id FROM memory_item WHERE id = 'delete-memory'",
        )
        .fetch_one(db.pool())
        .await
        .expect("historical memory survives Task/Execution teardown");
        assert_eq!(memory, ("body remains unchanged".to_owned(), None, None));
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE id = 'delete-project-event'",
        )
        .fetch_one(db.pool())
        .await
        .expect("domain-event history survives");
        assert_eq!(event_count, 1);
        let binding_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM project_agent_binding WHERE project_id = 'delete-project'",
        )
        .fetch_one(db.pool())
        .await
        .expect("Project Agent binding cascade");
        assert_eq!(binding_count, 0);
        let handoff_rows: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM agent_handoff WHERE id = 'delete-handoff')
                  + (SELECT COUNT(*) FROM agent_handoff_delivery
                     WHERE handoff_id = 'delete-handoff')",
        )
        .fetch_one(db.pool())
        .await
        .expect("Project Chat cascade removes scoped handoff history");
        assert_eq!(handoff_rows, 0);
        let chat_history_rows: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM agent_chat_message WHERE id = 'delete-chat-message')
                  + (SELECT COUNT(*) FROM agent_chat_turn_job WHERE id = 'delete-chat-turn')
                  + (SELECT COUNT(*) FROM agent_chat_instruction_revision
                     WHERE id = 'delete-chat-instruction')",
        )
        .fetch_one(db.pool())
        .await
        .expect("Project Chat history cascade");
        assert_eq!(chat_history_rows, 0);
        let os_rows: i64 = sqlx::query_scalar(
            "SELECT (SELECT COUNT(*) FROM project_charter WHERE project_id = 'delete-project')
                  + (SELECT COUNT(*) FROM project_execution_baseline WHERE project_id = 'delete-project')
                  + (SELECT COUNT(*) FROM project_milestone WHERE project_id = 'delete-project')",
        )
        .fetch_one(db.pool())
        .await
        .expect("Project OS delete contract");
        assert_eq!(os_rows, 0);
        let guard_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_deletion_guard")
            .fetch_one(db.pool())
            .await
            .expect("Project deletion guard cleanup");
        assert_eq!(guard_rows, 0);
        let fk_errors: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(db.pool())
            .await
            .expect("foreign key check");
        assert!(
            fk_errors.is_empty(),
            "deletion leaves valid FKs: {fk_errors:?}"
        );
    }
}
