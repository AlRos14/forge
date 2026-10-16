use db::{create_sqlite_pool, run_migrations_from};
use sqlx::SqlitePool;
use std::{fs, path::Path};

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

async fn seed_integration(
    pool: &SqlitePool,
    id: &str,
    assignee_type: Option<&str>,
    assignee_id: Option<&str>,
) {
    let project_id = format!("{id}-project");
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES (?, 'Integration defaults', '{}', '{}',
                 '2026-10-12T12:00:00Z', '2026-10-12T12:00:00Z')",
    )
    .bind(&project_id)
    .execute(pool)
    .await
    .expect("Project");
    sqlx::query(
        "INSERT INTO project_integration
         (id, project_id, platform, base_url, owner, repo, token_secret_ref,
          poll_interval_secs, sync_filter, default_assignee_type,
          default_assignee_id, enabled, created_at, updated_at)
         VALUES (?, ?, 'github', 'https://api.github.com',
                 'acme', ?, 'GITHUB_TOKEN', 300, '{}', ?, ?, 1,
                 '2026-10-12T12:00:00Z', '2026-10-12T12:00:00Z')",
    )
    .bind(id)
    .bind(project_id)
    .bind(id)
    .bind(assignee_type)
    .bind(assignee_id)
    .execute(pool)
    .await
    .expect("V120 integration row");
}

#[tokio::test]
async fn v123_maps_only_exact_integration_actor_refs_and_preserves_invalid_source_values() {
    let temp = tempfile::tempdir().expect("temp dir");
    let v122 = temp.path().join("v122");
    let v123 = temp.path().join("v123");
    fs::create_dir_all(&v122).expect("V122 migration dir");
    fs::create_dir_all(&v123).expect("V123 migration dir");
    copy_migrations_through(122, &v122);
    copy_migrations_through(123, &v123);

    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, &v122)
        .await
        .expect("V122 baseline");
    let now = "2026-10-12T12:00:00Z";
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('pr13-integration-human', 'pr13-human@example.test', 'fixture', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("Human Actor");
    sqlx::query(
        "INSERT INTO agent_identity (id, name, created_at, updated_at)
         VALUES ('pr13-integration-agent', 'Integration Actor', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(&pool)
    .await
    .expect("Agent Actor");

    seed_integration(
        &pool,
        "pr13-agent-default",
        Some("agent"),
        Some("pr13-integration-agent"),
    )
    .await;
    sqlx::query(
        "UPDATE project_integration SET default_task_state = 'in_progress'
         WHERE id = 'pr13-agent-default'",
    )
    .execute(&pool)
    .await
    .expect("historical Task state compatibility value");
    seed_integration(
        &pool,
        "pr13-human-default",
        Some("user"),
        Some("pr13-integration-human"),
    )
    .await;
    seed_integration(&pool, "pr13-no-default", None, None).await;
    seed_integration(&pool, "pr13-invalid-sentinel", Some("user"), Some("human")).await;
    seed_integration(
        &pool,
        "pr13-missing-actor",
        Some("agent"),
        Some("missing-agent"),
    )
    .await;
    seed_integration(
        &pool,
        "pr13-invalid-actor-id",
        Some("user"),
        Some(" pr13-integration-human "),
    )
    .await;

    run_migrations_from(&pool, &v123)
        .await
        .expect("V123 exact storage cutover");

    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT id, default_implementer_kind, default_implementer_id, token_secret_ref
         FROM project_integration ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("canonical integrations");
    assert_eq!(
        rows,
        vec![
            (
                "pr13-agent-default".to_owned(),
                Some("agent".to_owned()),
                Some("pr13-integration-agent".to_owned()),
                "GITHUB_TOKEN".to_owned(),
            ),
            (
                "pr13-human-default".to_owned(),
                Some("human".to_owned()),
                Some("pr13-integration-human".to_owned()),
                "GITHUB_TOKEN".to_owned(),
            ),
            (
                "pr13-invalid-actor-id".to_owned(),
                None,
                None,
                "GITHUB_TOKEN".to_owned(),
            ),
            (
                "pr13-invalid-sentinel".to_owned(),
                None,
                None,
                "GITHUB_TOKEN".to_owned(),
            ),
            (
                "pr13-missing-actor".to_owned(),
                None,
                None,
                "GITHUB_TOKEN".to_owned(),
            ),
            (
                "pr13-no-default".to_owned(),
                None,
                None,
                "GITHUB_TOKEN".to_owned(),
            ),
        ]
    );

    let issues: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "SELECT integration_id, source_kind, source_id, issue_kind
         FROM pr13_integration_default_implementer_issue ORDER BY integration_id",
    )
    .fetch_all(&pool)
    .await
    .expect("invalid source values remain in provenance ledger");
    assert_eq!(
        issues,
        vec![
            (
                "pr13-invalid-actor-id".to_owned(),
                Some("user".to_owned()),
                Some(" pr13-integration-human ".to_owned()),
                "invalid_actor_id".to_owned(),
            ),
            (
                "pr13-invalid-sentinel".to_owned(),
                Some("user".to_owned()),
                Some("human".to_owned()),
                "reserved_human_sentinel".to_owned(),
            ),
            (
                "pr13-missing-actor".to_owned(),
                Some("agent".to_owned()),
                Some("missing-agent".to_owned()),
                "agent_not_found".to_owned(),
            ),
        ]
    );

    let archived_default_task_state: Option<String> = sqlx::query_scalar(
        "SELECT default_task_state FROM pr13_project_integration_legacy_defaults
         WHERE integration_id = 'pr13-agent-default'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact legacy workflow value remains archived");
    assert_eq!(archived_default_task_state.as_deref(), Some("in_progress"));

    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('project_integration') ORDER BY cid",
    )
    .fetch_all(&pool)
    .await
    .expect("target integration schema");
    assert!(columns.contains(&"default_implementer_kind".to_owned()));
    assert!(columns.contains(&"default_implementer_id".to_owned()));
    assert!(!columns.contains(&"default_assignee_type".to_owned()));
    assert!(!columns.contains(&"default_assignee_id".to_owned()));
    assert!(!columns.contains(&"default_task_state".to_owned()));
    let foreign_key_violations: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_foreign_key_check")
            .fetch_one(&pool)
            .await
            .expect("foreign key check");
    assert_eq!(foreign_key_violations, 0);
}
