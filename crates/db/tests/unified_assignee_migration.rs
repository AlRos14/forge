use db::{create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, run_migrations_from};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tempfile::TempDir;

fn copy_migrations(source: &Path, target: &Path, first: i64, last: i64) -> PathBuf {
    fs::create_dir_all(target).expect("migration stage directory creates");
    for entry in fs::read_dir(source).expect("migration source reads") {
        let entry = entry.expect("migration entry reads");
        let path = entry.path();
        let Some(filename) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(version) = filename
            .strip_prefix('V')
            .and_then(|name| name.split_once("__"))
            .and_then(|(version, _)| version.parse::<i64>().ok())
        else {
            continue;
        };
        if (first..=last).contains(&version) {
            fs::copy(&path, target.join(filename)).expect("migration copies into stage");
        }
    }
    target.to_path_buf()
}

async fn seed_pre_pr13_singleton_row(
    pool: &sqlx::SqlitePool,
) -> (String, String, String, String, String) {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let assignment_id = new_uuid_v4();
    let actor_ref = "opaque-retired-agent-reference".to_owned();
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES (?, 'Forge', '{}', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .expect("project inserts");
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at)
         VALUES (?, ?, 'forge', 'https://example.test/forge.git', NULL, 'direct_merge', 'main', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .expect("repo inserts");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at)
         VALUES (?, ?, ?, 'Historical migration fixture', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .expect("task inserts");
    sqlx::query(
        "INSERT INTO task_role_assignment
         (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
         VALUES (?, ?, 'legacy-role', 'agent', ?, ?, ?)",
    )
    .bind(&assignment_id)
    .bind(&task_id)
    .bind(&actor_ref)
    .bind(&now)
    .bind(&now)
    .execute(pool)
    .await
    .expect("pre-cutover singleton history inserts");
    (assignment_id, task_id, actor_ref, now.clone(), now)
}

#[tokio::test]
async fn v120_upgrade_archives_exact_singleton_rows_and_drops_runtime_projections() {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    let temp = TempDir::new().expect("migration fixture directory creates");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let v120 = copy_migrations(&source, &temp.path().join("v120"), 1, 120);
    let v121_v126 = copy_migrations(&source, &temp.path().join("v121-v126"), 121, 126);
    let v127 = copy_migrations(&source, &temp.path().join("v127"), 127, 127);
    let v128 = copy_migrations(&source, &temp.path().join("v128"), 128, 128);

    run_migrations_from(&pool, &v120)
        .await
        .expect("V120 applies");
    let (assignment_id, task_id, actor_ref, created_at, updated_at) =
        seed_pre_pr13_singleton_row(&pool).await;
    run_migrations_from(&pool, &v121_v126)
        .await
        .expect("V121-V126 upgrade V120");
    run_migrations_from(&pool, &v127)
        .await
        .expect("V127 archives and drops singleton storage");
    let projection_updated_at = now_rfc3339();
    sqlx::query(
        "UPDATE task SET assignee_type = 'agent', assignee_id = 'exact-residual-actor',
                         updated_at = ?, version = version + 1 WHERE id = ?",
    )
    .bind(&projection_updated_at)
    .bind(&task_id)
    .execute(&pool)
    .await
    .expect("pre-V128 legacy projection fixture updates");
    run_migrations_from(&pool, &v128)
        .await
        .expect("V128 archives and drops Task singleton columns");

    let archived: (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
        String,
    ) = sqlx::query_as(
        "SELECT id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at
             FROM task_role_assignment_history WHERE id = ?",
    )
    .bind(&assignment_id)
    .fetch_one(&pool)
    .await
    .expect("exact historical row remains");
    assert_eq!(
        archived,
        (
            assignment_id,
            task_id.clone(),
            "legacy-role".to_owned(),
            Some("agent".to_owned()),
            Some(actor_ref),
            created_at,
            updated_at,
        )
    );
    let runtime_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_role_assignment'",
    )
    .fetch_one(&pool)
    .await
    .expect("runtime table check runs");
    assert_eq!(runtime_table, 0);
    let task_projection: (String, String, i64, String) = sqlx::query_as(
        "SELECT task_id, assignee_id, task_version, task_updated_at
         FROM pr13_task_assignment_projection_history WHERE task_id = ?",
    )
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .expect("exact Task assignment projection remains as history");
    assert_eq!(
        task_projection,
        (
            task_id.clone(),
            "exact-residual-actor".to_owned(),
            2,
            projection_updated_at
        )
    );
    let task_columns: Vec<String> =
        sqlx::query_scalar("SELECT name FROM pragma_table_info('task')")
            .fetch_all(&pool)
            .await
            .expect("Task columns load");
    assert!(!task_columns.iter().any(|column| column == "assignee_type"));
    assert!(!task_columns.iter().any(|column| column == "assignee_id"));
    let fabricated_history = sqlx::query(
        "INSERT INTO pr13_task_assignment_projection_history
         (task_id, assignee_type, assignee_id, task_version, task_created_at,
          task_updated_at, captured_at)
         VALUES ('fabricated', 'agent', 'invented', 1, 'now', 'now', 'now')",
    )
    .execute(&pool)
    .await;
    assert!(fabricated_history.is_err(), "history is migration-only");
    let foreign_key_violations: Vec<(String, i64, String, i64)> =
        sqlx::query_as("PRAGMA foreign_key_check")
            .fetch_all(&pool)
            .await
            .expect("foreign key check runs");
    assert!(
        foreign_key_violations.is_empty(),
        "{foreign_key_violations:?}"
    );
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity check runs");
    assert_eq!(integrity, "ok");
}

#[tokio::test]
async fn fresh_and_empty_v120_upgrade_have_no_singleton_runtime_projections() {
    let fresh = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("fresh pool creates");
    run_migrations(&fresh)
        .await
        .expect("fresh migrations apply");

    let upgrade = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("upgrade pool creates");
    let temp = TempDir::new().expect("migration fixture directory creates");
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let v120 = copy_migrations(&source, &temp.path().join("v120"), 1, 120);
    let v121_v128 = copy_migrations(&source, &temp.path().join("v121-v128"), 121, 128);
    run_migrations_from(&upgrade, &v120)
        .await
        .expect("V120 applies");
    run_migrations_from(&upgrade, &v121_v128)
        .await
        .expect("empty V120 upgrade applies");

    for pool in [&fresh, &upgrade] {
        let runtime_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_role_assignment'",
        )
        .fetch_one(pool)
        .await
        .expect("runtime table check runs");
        assert_eq!(runtime_table, 0);
        let history_table: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'task_role_assignment_history'",
        )
        .fetch_one(pool)
        .await
        .expect("history table check runs");
        assert_eq!(history_table, 1);
        let task_columns: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_table_info('task')")
                .fetch_all(pool)
                .await
                .expect("Task columns load");
        assert!(!task_columns.iter().any(|column| column == "assignee_type"));
        assert!(!task_columns.iter().any(|column| column == "assignee_id"));
    }
}

#[tokio::test]
async fn task_roles_preserve_multi_actor_membership_and_history() {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations apply");

    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let human_a = new_uuid_v4();
    let human_b = new_uuid_v4();
    let implementer_role_id = new_uuid_v4();
    let planner_role_id = new_uuid_v4();
    let member_a = new_uuid_v4();
    let member_b = new_uuid_v4();
    let planner_member_a = new_uuid_v4();

    sqlx::query(
        "INSERT INTO project (id, name, settings, created_at, updated_at) VALUES (?, 'Forge', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("project inserts");
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at) VALUES (?, ?, 'forge', 'https://example.com/forge.git', NULL, 'direct_merge', 'main', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("repo inserts");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at) VALUES (?, ?, ?, 'Task', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("task inserts");

    for (id, email) in [
        (&human_a, "human-a@example.com"),
        (&human_b, "human-b@example.com"),
    ] {
        sqlx::query(
            "INSERT INTO user (id, email, password_hash, created_at, updated_at) VALUES (?, ?, 'test', ?, ?)",
        )
        .bind(id)
        .bind(email)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("human inserts");
    }

    for (id, role) in [
        (&implementer_role_id, "implementer"),
        (&planner_role_id, "planner"),
    ] {
        sqlx::query(
            "INSERT INTO task_role (id, task_id, role, coordination_mode, policy_json, version, created_at, updated_at) VALUES (?, ?, ?, 'collaborative', '{}', 1, ?, ?)",
        )
        .bind(id)
        .bind(&task_id)
        .bind(role)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("TaskRole inserts");
    }

    for (id, role_id, actor_id) in [
        (&member_a, &implementer_role_id, &human_a),
        (&member_b, &implementer_role_id, &human_b),
        (&planner_member_a, &planner_role_id, &human_a),
    ] {
        sqlx::query(
            "INSERT INTO role_membership (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at) VALUES (?, ?, 'human', ?, 'active', 1, ?, ?, NULL)",
        )
        .bind(id)
        .bind(role_id)
        .bind(actor_id)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("membership inserts");
    }

    let implementer_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM role_membership WHERE task_role_id = ? AND status = 'active'",
    )
    .bind(&implementer_role_id)
    .fetch_one(&pool)
    .await
    .expect("membership count loads");
    assert_eq!(implementer_count, 2);

    let duplicate = sqlx::query(
        "INSERT INTO role_membership (id, task_role_id, actor_kind, actor_id, status, version, created_at, updated_at, ended_at) VALUES (?, ?, 'human', ?, 'active', 1, ?, ?, NULL)",
    )
    .bind(new_uuid_v4())
    .bind(&implementer_role_id)
    .bind(&human_a)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(duplicate.is_err());

    let ended_at = now_rfc3339();
    sqlx::query(
        "UPDATE role_membership SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1 WHERE id = ? AND version = 1",
    )
    .bind(&ended_at)
    .bind(&ended_at)
    .bind(&member_a)
    .execute(&pool)
    .await
    .expect("membership ends");

    let remaining: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM role_membership WHERE task_role_id = ? AND status = 'active'",
    )
    .bind(&implementer_role_id)
    .fetch_one(&pool)
    .await
    .expect("remaining membership loads");
    assert_eq!(remaining, 1);

    let historical_status: String =
        sqlx::query_scalar("SELECT status FROM role_membership WHERE id = ?")
            .bind(&member_a)
            .fetch_one(&pool)
            .await
            .expect("historical membership loads");
    assert_eq!(historical_status, "ended");

    let reopen = sqlx::query(
        "UPDATE role_membership SET status = 'active', ended_at = NULL, version = version + 1 WHERE id = ? AND version = 2",
    )
    .bind(&member_a)
    .execute(&pool)
    .await;
    assert!(reopen.is_err());

    let cross_role_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM role_membership WHERE actor_kind = 'human' AND actor_id = ? AND status = 'active'",
    )
    .bind(&human_a)
    .fetch_one(&pool)
    .await
    .expect("cross-role membership loads");
    assert_eq!(cross_role_count, 1);

    // Physical Human deletion closes exact target memberships and preserves
    // their rows. No singleton projection is rebuilt from the surviving actor.
    sqlx::query("DELETE FROM user WHERE id = ?")
        .bind(&human_a)
        .execute(&pool)
        .await
        .expect("human deletion closes memberships");
    let human_a_states: Vec<String> = sqlx::query_scalar(
        "SELECT status FROM role_membership WHERE actor_kind = 'human' AND actor_id = ? ORDER BY id",
    )
    .bind(&human_a)
    .fetch_all(&pool)
    .await
    .expect("deleted Human membership history loads");
    assert_eq!(human_a_states.len(), 2);
    assert!(human_a_states.iter().all(|status| status == "ended"));
    let human_b_active: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM role_membership WHERE actor_kind = 'human' AND actor_id = ? AND status = 'active'",
    )
    .bind(&human_b)
    .fetch_one(&pool)
    .await
    .expect("surviving Human membership loads");
    assert_eq!(human_b_active, 1);
}
