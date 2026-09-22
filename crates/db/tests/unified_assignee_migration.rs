use db::{create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations};

#[tokio::test]
async fn all_migrations_apply_and_task_role_assignment_post_sweep_shape_is_valid() {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations(&pool).await.expect("migrations apply");

    let applied_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration")
        .fetch_one(&pool)
        .await
        .expect("migration count loads");
    let expected_count = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/migrations"))
        .expect("migration directory reads")
        .filter(|entry| {
            entry
                .as_ref()
                .ok()
                .and_then(|entry| entry.path().file_name()?.to_str().map(str::to_owned))
                .is_some_and(|filename| filename.starts_with('V') && filename.ends_with(".sql"))
        })
        .count() as i64;
    assert_eq!(applied_count, expected_count);

    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();

    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at) VALUES (?, 'Forge', '{}', '{}', ?, ?)",
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

    let invalid_insert = sqlx::query(
        "INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at) VALUES (?, ?, 'coder', 'agent', NULL, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(invalid_insert.is_err());

    let assignment_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at) VALUES (?, ?, 'coder', 'agent', 'agent-a', ?, ?)",
    )
    .bind(&assignment_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("valid role assignment inserts");

    sqlx::query("UPDATE task_role_assignment SET assignee_id = NULL WHERE id = ?")
        .bind(&assignment_id)
        .execute(&pool)
        .await
        .expect("post-sweep update accepts deleted agent marker");

    let assignee_id: Option<String> =
        sqlx::query_scalar("SELECT assignee_id FROM task_role_assignment WHERE id = ?")
            .bind(&assignment_id)
            .fetch_one(&pool)
            .await
            .expect("assignee_id loads");
    assert_eq!(assignee_id, None);
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
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at) VALUES (?, 'Forge', '{}', '{}', ?, ?)",
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

    // Physical Human deletion closes history and rebuilds the bounded
    // singleton projection from the surviving membership, rather than
    // leaving a deleted identity visible through legacy display data.
    sqlx::query(
        "INSERT INTO task_role_assignment (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at) VALUES (?, ?, 'coder', 'user', ?, ?, ?)",
    )
    .bind(new_uuid_v4())
    .bind(&task_id)
    .bind(&human_a)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy projection inserts");
    sqlx::query("UPDATE task SET assignee_type = 'user', assignee_id = ? WHERE id = ?")
        .bind(&human_a)
        .bind(&task_id)
        .execute(&pool)
        .await
        .expect("task projection inserts");
    sqlx::query("DELETE FROM user WHERE id = ?")
        .bind(&human_a)
        .execute(&pool)
        .await
        .expect("human deletion closes memberships");

    let surviving_projection: (String, String) = sqlx::query_as(
        "SELECT assignee_type, assignee_id FROM task_role_assignment WHERE task_id = ? AND role_name = 'coder'",
    )
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .expect("surviving legacy projection loads");
    assert_eq!(surviving_projection, ("user".to_owned(), human_b.clone()));
    let surviving_task_projection: (String, String) =
        sqlx::query_as("SELECT assignee_type, assignee_id FROM task WHERE id = ?")
            .bind(&task_id)
            .fetch_one(&pool)
            .await
            .expect("surviving task projection loads");
    assert_eq!(surviving_task_projection, ("user".to_owned(), human_b));
}
