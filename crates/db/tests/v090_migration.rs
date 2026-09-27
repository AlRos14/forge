use std::{fs, path::Path};

use db::{create_sqlite_pool, now_rfc3339, run_migrations_from};
use sqlx::Row;

fn copy_migrations_through(limit: i64, destination: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in fs::read_dir(source).expect("migration directory") {
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

async fn snapshot_row(pool: &sqlx::SqlitePool, table: &str, id: &str) -> String {
    let table_info = format!("PRAGMA table_info(\"{table}\")");
    let columns = sqlx::query(&table_info)
        .fetch_all(pool)
        .await
        .expect("table metadata")
        .into_iter()
        .map(|row| row.try_get::<String, _>("name").expect("column name"))
        .collect::<Vec<_>>();
    let pairs = columns
        .iter()
        .map(|column| format!("'{column}', \"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let query = format!("SELECT json_object({pairs}) FROM \"{table}\" WHERE id = ?");
    sqlx::query_scalar(&query)
        .bind(id)
        .fetch_one(pool)
        .await
        .expect("complete row snapshot")
}

#[tokio::test]
async fn v090_applies_over_v089_and_preserves_legacy_rows_after_reopen() {
    let temp = tempfile::tempdir().expect("temp dir");
    let migration_dir = temp.path().join("migrations");
    fs::create_dir_all(&migration_dir).expect("migration dir");
    copy_migrations_through(89, &migration_dir);

    let database_path = temp.path().join("forge-v089.db");
    let database_url = format!("sqlite://{}", database_path.display());
    let pool = create_sqlite_pool(&database_url).await.expect("pool");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("schema through V089");

    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES ('v090-project', 'V090 preservation', '{}', '{}', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy project");
    sqlx::query(
        "INSERT INTO repo (
             id, project_id, name, remote_url, local_path, work_mode,
             default_branch, created_at, updated_at
         ) VALUES ('v090-repo', 'v090-project', 'repo',
                   'https://example.invalid/v090.git', NULL, 'direct_merge', 'main', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy repo");
    sqlx::query(
        "INSERT INTO task (
             id, project_id, repo_id, title, task_type, status, created_at, updated_at
         ) VALUES ('v090-task', 'v090-project', 'v090-repo', 'legacy plan task',
                   'planning', 'in_progress', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy task");
    sqlx::query(
        "INSERT INTO task_plan_revision (
             id, task_id, revision, checkpoint, markdown, content_digest,
             checklist_json, warnings_json, source_execution_id, created_at
         ) VALUES ('v090-plan', 'v090-task', 1, 'approved', '# Existing plan',
                   'sha256:legacy', '[]', '[]', NULL, ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy plan revision");
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('v090-legacy-user', 'v090-legacy@example.test', 'fixture', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy chat user");
    let main_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'account_main' AND account_id = 'v090-legacy-user'",
    )
    .fetch_one(&pool)
    .await
    .expect("User insert creates the legacy Main Agent Chat");
    let project_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'project' AND project_id = 'v090-project'",
    )
    .fetch_one(&pool)
    .await
    .expect("Project insert creates the legacy Project Agent Chat");
    sqlx::query(
        "INSERT INTO agent_chat_message (
             id, chat_id, sequence, author_type, author_id, content, status,
             correlation_id, source_type, created_at
         ) VALUES ('v090-legacy-message', ?, 1, 'user',
                   'v090-legacy-user', 'legacy chat body', 'complete',
                   'v090-chat-correlation', 'native', ?)",
    )
    .bind(&main_chat_id)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy Agent Chat message");
    sqlx::query(
        "INSERT INTO agent_handoff (
             id, source_chat_id, target_chat_id, content, correlation_id,
             dedupe_key, created_at, updated_at
         ) VALUES ('v090-legacy-handoff', ?, ?,
                   'legacy handoff body', 'v090-handoff-correlation',
                   'v090-handoff-dedupe', ?, ?)",
    )
    .bind(&main_chat_id)
    .bind(&project_chat_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy Agent Handoff");
    sqlx::query(
        "INSERT INTO project_decision (
             id, project_id, state, decision_class, question, context_json,
             options_json, selected_outcome, rationale, principal_type,
             principal_id, authority_basis, authorization_action, explicit_event,
             authorization_occurred_at, source_refs_json, affected_records_json, created_at
         ) VALUES ('v090-legacy-decision', 'v090-project', 'active',
                   'project_implementation', 'legacy question', '{}', '[]',
                   'keep', 'legacy rationale', 'human', 'v090-legacy-user',
                   'legacy-authority', 'project.decision.record',
                   'v090-legacy-event', ?, '[]', '{}', ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy Project Decision");
    let before_plan = snapshot_row(&pool, "task_plan_revision", "v090-plan").await;
    let before_chat_message =
        snapshot_row(&pool, "agent_chat_message", "v090-legacy-message").await;
    let before_agent_handoff = snapshot_row(&pool, "agent_handoff", "v090-legacy-handoff").await;
    let before_project_decision =
        snapshot_row(&pool, "project_decision", "v090-legacy-decision").await;

    let migration_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    fs::copy(
        migration_root.join("V090__generic_collaboration_primitives.sql"),
        migration_dir.join("V090__generic_collaboration_primitives.sql"),
    )
    .expect("V090 copied");
    run_migrations_from(&pool, &migration_dir)
        .await
        .expect("V090 applies over V089");
    pool.close().await;

    let reopened = create_sqlite_pool(&database_url)
        .await
        .expect("database reopens");
    assert_eq!(
        snapshot_row(&reopened, "task_plan_revision", "v090-plan").await,
        before_plan
    );
    assert_eq!(
        snapshot_row(&reopened, "agent_chat_message", "v090-legacy-message").await,
        before_chat_message
    );
    assert_eq!(
        snapshot_row(&reopened, "agent_handoff", "v090-legacy-handoff").await,
        before_agent_handoff
    );
    assert_eq!(
        snapshot_row(&reopened, "project_decision", "v090-legacy-decision").await,
        before_project_decision
    );
    let latest: i64 = sqlx::query_scalar("SELECT MAX(version) FROM _migration")
        .fetch_one(&reopened)
        .await
        .expect("latest migration number");
    assert_eq!(latest, 90);
    let foreign_key_issues: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&reopened)
            .await
            .expect("FK check");
    assert!(foreign_key_issues.is_empty());
}
