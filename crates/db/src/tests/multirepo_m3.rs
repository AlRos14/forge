use super::{
    copy_migrations_through, insert_test_work_unit, multirepo_seed_repo, multirepo_seed_task,
    seed_agent, seed_project, seed_task, sqlite_db,
};
use crate::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations_from, CreateWorkspace, DbError,
    TaskRepositoryMergeBatch, TaskRepositoryMergeBatchRepo, TaskRepositoryMergeBatchRepoRow,
    TaskRepositoryMergeBatchRepoState, TaskRepositoryMergeBatchState, TaskRepositoryRole,
    TaskRepositorySetRepo, TaskRepositoryWrite, WorkspaceRepo, WorkspaceStatus,
};
use sqlx::{Row, SqlitePool};
use std::path::Path;

async fn legacy_single_repo_pr_fixture() -> (SqlitePool, String, String, String, String, String) {
    let migrations = tempfile::tempdir().expect("legacy migration directory creates");
    copy_migrations_through(108, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("migrations through V108 run");

    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let other_repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    let metadata_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES (?, 'V183 legacy Project', '{}', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy Project inserts");
    for (id, name) in [(&repo_id, "legacy-primary"), (&other_repo_id, "other-repo")] {
        sqlx::query(
            "INSERT INTO repo (id, project_id, name, remote_url, default_branch,
                               created_at, updated_at)
             VALUES (?, ?, ?, ?, 'main', ?, ?)",
        )
        .bind(id)
        .bind(&project_id)
        .bind(name)
        .bind(format!("https://example.com/{name}.git"))
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("legacy repository inserts");
    }
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at)
         VALUES (?, ?, ?, 'V183 legacy Task', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy Task inserts");
    sqlx::query(
        "INSERT INTO pr_metadata (
             id, task_id, provider_type, provider_pr_id, pr_url, source_branch,
             target_branch, pr_state, merge_status, last_synced_at, created_at, updated_at
         ) VALUES (?, ?, 'github', 'PR-42', 'https://example.com/pr/42',
                   'feature/legacy', 'main', 'open', 'pending', ?, ?, ?)",
    )
    .bind(&metadata_id)
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("pre-admission legacy PR metadata inserts");

    (
        pool,
        project_id,
        repo_id,
        other_repo_id,
        task_id,
        metadata_id,
    )
}

async fn migrate_to_latest(pool: &SqlitePool) {
    let migrations = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    run_migrations_from(pool, migrations)
        .await
        .expect("remaining migrations run");
}

async fn assert_foreign_key_check_is_clear(pool: &SqlitePool) {
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(pool)
        .await
        .expect("foreign key check runs");
    assert!(violations.is_empty(), "migration leaves no FK violations");
}

#[tokio::test]
async fn multirepo_pr_v183_preserves_single_repo_metadata() {
    let (pool, _, repo_id, _, task_id, metadata_id) = legacy_single_repo_pr_fixture().await;
    migrate_to_latest(&pool).await;

    let row = sqlx::query(
        "SELECT id, task_id, repo_id, provider_type, provider_pr_id, pr_url, source_branch,
                target_branch, pr_state, merge_status, last_synced_at, created_at, updated_at,
                task_merge_operation_id, publish_operation_id, admission_status
         FROM pr_metadata WHERE id = ?",
    )
    .bind(&metadata_id)
    .fetch_one(&pool)
    .await
    .expect("legacy PR metadata remains readable");
    assert_eq!(row.try_get::<String, _>("id").unwrap(), metadata_id);
    assert_eq!(row.try_get::<String, _>("task_id").unwrap(), task_id);
    assert_eq!(row.try_get::<String, _>("repo_id").unwrap(), repo_id);
    assert_eq!(row.try_get::<String, _>("provider_type").unwrap(), "github");
    assert_eq!(row.try_get::<String, _>("provider_pr_id").unwrap(), "PR-42");
    assert_eq!(
        row.try_get::<String, _>("pr_url").unwrap(),
        "https://example.com/pr/42"
    );
    assert_eq!(
        row.try_get::<String, _>("source_branch").unwrap(),
        "feature/legacy"
    );
    assert_eq!(row.try_get::<String, _>("target_branch").unwrap(), "main");
    assert_eq!(row.try_get::<String, _>("pr_state").unwrap(), "open");
    assert_eq!(row.try_get::<String, _>("merge_status").unwrap(), "pending");
    assert_eq!(
        row.try_get::<Option<String>, _>("last_synced_at").unwrap(),
        Some(row.try_get::<String, _>("created_at").unwrap())
    );
    assert_eq!(
        row.try_get::<String, _>("created_at").unwrap(),
        row.try_get::<String, _>("updated_at").unwrap()
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("task_merge_operation_id")
            .unwrap(),
        None
    );
    assert_eq!(
        row.try_get::<Option<String>, _>("publish_operation_id")
            .unwrap(),
        None
    );
    assert_eq!(
        row.try_get::<String, _>("admission_status").unwrap(),
        "legacy_unadmitted"
    );
    assert_foreign_key_check_is_clear(&pool).await;
}

#[tokio::test]
async fn multirepo_pr_v183_preserves_cross_guards_recreates_attached() {
    let (pool, _, _, other_repo_id, _, metadata_id) = legacy_single_repo_pr_fixture().await;
    migrate_to_latest(&pool).await;

    let triggers = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master
         WHERE type = 'trigger' AND tbl_name = 'pr_metadata' ORDER BY name",
    )
    .fetch_all(&pool)
    .await
    .expect("PR metadata triggers load");
    for expected in [
        "pr_metadata_merge_admission_guard_insert",
        "pr_metadata_merge_admission_guard_update",
        "pr_metadata_admission_status_guard",
        "pr_metadata_repo_insert_guard",
        "pr_metadata_repo_immutable",
    ] {
        assert!(
            triggers.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }

    let repo_rebind = sqlx::query("UPDATE pr_metadata SET repo_id = ? WHERE id = ?")
        .bind(&other_repo_id)
        .bind(&metadata_id)
        .execute(&pool)
        .await;
    assert!(
        repo_rebind.is_err(),
        "PR metadata repo identity is immutable"
    );

    let unadmitted_status =
        sqlx::query("UPDATE pr_metadata SET admission_status = 'open' WHERE id = ?")
            .bind(&metadata_id)
            .execute(&pool)
            .await;
    assert!(
        unadmitted_status.is_err(),
        "attached admission provenance guard remains active"
    );

    let invalid_rebind =
        sqlx::query("UPDATE pr_metadata SET task_merge_operation_id = ? WHERE id = ?")
            .bind(new_uuid_v4())
            .bind(&metadata_id)
            .execute(&pool)
            .await;
    assert!(
        invalid_rebind.is_err(),
        "attached operation admission guard remains active"
    );

    let invalid_insert = sqlx::query(
        "INSERT INTO pr_metadata (
             id, task_id, repo_id, provider_type, source_branch, target_branch,
             created_at, updated_at
         ) SELECT ?, task_id, ?, 'github', 'feature/other', 'main', created_at, updated_at
           FROM pr_metadata WHERE id = ?",
    )
    .bind(new_uuid_v4())
    .bind(&other_repo_id)
    .bind(&metadata_id)
    .execute(&pool)
    .await;
    assert!(
        invalid_insert.is_err(),
        "cross-table admission guard rejects an unadmitted PR"
    );
    let cross_guards = sqlx::query_scalar::<_, String>(
        "SELECT name FROM sqlite_master
         WHERE type = 'trigger' AND name IN (
             'pr_rework_supersession_insert_guard',
             'remote_pr_history_insert_guard',
             'task_integration_operation_provider_result_guard'
         )",
    )
    .fetch_all(&pool)
    .await
    .expect("cross-table trigger names load");
    assert_eq!(
        cross_guards.len(),
        3,
        "cross-table guards survive table rebuild"
    );
    assert_foreign_key_check_is_clear(&pool).await;
}

#[tokio::test]
async fn multirepo_integration_repository_revision_pins_existing_work_units() {
    let migrations = tempfile::tempdir().expect("V182 migration directory creates");
    copy_migrations_through(182, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("pool creates");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("migrations through V182 run");
    let db = crate::SqliteDb::new(pool.clone());
    let project_id = seed_project(&db, "M3 pin migration", None).await;
    let repo_id = multirepo_seed_repo(&db, &project_id, "pin-primary").await;
    let agent_id = seed_agent(&db, "M3 pin agent", "global", None).await;
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        Some(&agent_id),
        "working".to_owned(),
        "Historical WorkUnit pin",
    )
    .await;
    let work_unit_id = insert_test_work_unit(
        &db,
        &task_id,
        &agent_id,
        "implementer",
        true,
        Some(&agent_id),
    )
    .await;

    let now = now_rfc3339();
    let workspace_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO workspace (
             id, task_id, repo_id, worktree_path, branch, status, before_sha,
             created_at, updated_at
         ) VALUES (?, ?, ?, '/tmp/m3-integration', 'integration/main', 'ready', 'base-sha', ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&task_id)
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical integration workspace inserts");
    sqlx::query(
        "INSERT INTO workspace_scope (
             workspace_id, task_id, scope_kind, work_unit_id, created_at, workspace_group_id
         ) VALUES (?, ?, 'integration', NULL, ?, NULL)",
    )
    .bind(&workspace_id)
    .bind(&task_id)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical integration scope inserts");

    migrate_to_latest(&pool).await;

    let pin: i64 = sqlx::query_scalar(
        "SELECT revision FROM work_unit_repository_set_revision
         WHERE work_unit_id = ? AND task_id = ?",
    )
    .bind(&work_unit_id)
    .bind(&task_id)
    .fetch_one(&pool)
    .await
    .expect("existing WorkUnit has a revision pin");
    assert_eq!(pin, 1);
    let pinned_revision = TaskRepositorySetRepo::get_for_work_unit(&db, &work_unit_id, &task_id)
        .await
        .expect("pinned revision loads")
        .expect("WorkUnit pin exists");
    assert_eq!(pinned_revision.revision, 1);
    assert_eq!(pinned_revision.repositories.len(), 1);
    assert_eq!(pinned_revision.repositories[0].repo_id, repo_id);

    let scope_repo: String = sqlx::query_scalar(
        "SELECT repo_id FROM workspace_scope WHERE workspace_id = ? AND scope_kind = 'integration'",
    )
    .bind(&workspace_id)
    .fetch_one(&pool)
    .await
    .expect("workspace scope repo is backfilled");
    assert_eq!(scope_repo, repo_id);
    let integration_workspace =
        WorkspaceRepo::get_integration_by_task_repo(&db, &task_id, &repo_id)
            .await
            .expect("repo-scoped integration workspace lookup succeeds")
            .expect("existing integration workspace remains discoverable");
    assert_eq!(integration_workspace.id, workspace_id);
    let integrations = WorkspaceRepo::list_integrations_by_task(&db, &task_id)
        .await
        .expect("integration workspaces list by Task");
    assert_eq!(integrations.len(), 1);
    assert_eq!(integrations[0].id, workspace_id);
    assert_foreign_key_check_is_clear(&pool).await;
}

#[tokio::test]
async fn multirepo_lifecycle_batch_state_is_monotonic() {
    let db = sqlite_db().await;
    let project_id = seed_project(&db, "M3 batch state", None).await;
    let repo_id = multirepo_seed_repo(&db, &project_id, "batch-primary").await;
    let task_id = multirepo_seed_task(
        &db,
        &project_id,
        &repo_id,
        vec![TaskRepositoryWrite {
            repo_id: repo_id.clone(),
            role: TaskRepositoryRole::Primary,
        }],
    )
    .await;
    let revision = TaskRepositorySetRepo::get_current(&db, &task_id)
        .await
        .expect("current revision loads")
        .expect("Task revision exists");
    let repository_set_digest = revision
        .digest
        .clone()
        .expect("new repository set has a digest");
    let now = now_rfc3339();
    let gate_id = new_uuid_v4();
    let evaluation_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO gate (
             id, task_id, gate_kind, scope_kind, scope_id, active_policy_revision, created_at
         ) VALUES (?, ?, 'delivery_acceptance', 'task', ?, NULL, ?)",
    )
    .bind(&gate_id)
    .bind(&task_id)
    .bind(&task_id)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("Task Gate inserts");
    sqlx::query(
        "INSERT INTO gate_policy_revision (
             gate_id, revision, schema_version, policy_json, policy_digest, created_at
         ) VALUES (?, 1, 1, '{}', ?, ?)",
    )
    .bind(&gate_id)
    .bind("a".repeat(64))
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("Gate policy revision inserts");
    sqlx::query("UPDATE gate SET active_policy_revision = 1 WHERE id = ?")
        .bind(&gate_id)
        .execute(db.pool())
        .await
        .expect("Gate policy activates");
    sqlx::query(
        "INSERT INTO gate_evaluation (
             id, gate_id, task_id, policy_revision, outcome, input_digest, result_json, evaluated_at
         ) VALUES (?, ?, ?, 1, 'satisfied', ?, '{}', ?)",
    )
    .bind(&evaluation_id)
    .bind(&gate_id)
    .bind(&task_id)
    .bind("b".repeat(64))
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("satisfied GateEvaluation inserts");

    let batch_id = new_uuid_v4();
    let batch = TaskRepositoryMergeBatch {
        id: batch_id.clone(),
        task_id: task_id.clone(),
        gate_evaluation_id: evaluation_id,
        repository_set_revision: revision.revision,
        repository_set_digest: Some(repository_set_digest),
        state: TaskRepositoryMergeBatchState::Integrating,
        version: 1,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    let repo_row = TaskRepositoryMergeBatchRepoRow {
        batch_id: batch_id.clone(),
        task_id: task_id.clone(),
        repository_set_revision: revision.revision,
        repo_id: repo_id.clone(),
        position: 0,
        state: TaskRepositoryMergeBatchRepoState::Pending,
        work_unit_repository_integration_id: None,
        task_merge_operation_id: None,
        pr_metadata_id: None,
        before_sha: None,
        after_sha: None,
        created_at: now.clone(),
        updated_at: now,
    };
    TaskRepositoryMergeBatchRepo::create(&db, &batch, &[repo_row.clone()])
        .await
        .expect("valid exact-revision batch creates");
    TaskRepositoryMergeBatchRepo::transition_repo(
        &db,
        &batch_id,
        &repo_id,
        1,
        TaskRepositoryMergeBatchRepoState::Integrated,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .expect("repository row advances to integrated");
    let completed = TaskRepositoryMergeBatchRepo::transition_batch(
        &db,
        &batch_id,
        2,
        TaskRepositoryMergeBatchState::Completed,
        &now_rfc3339(),
    )
    .await
    .expect("all integrated repositories allow batch completion");
    assert_eq!(completed.state, TaskRepositoryMergeBatchState::Completed);
    assert_eq!(completed.version, 3);
    let replayed = TaskRepositoryMergeBatchRepo::create(&db, &batch, &[repo_row])
        .await
        .expect("batch creation replay returns its changed durable state");
    assert_eq!(replayed, completed);

    let backwards = TaskRepositoryMergeBatchRepo::transition_batch(
        &db,
        &batch_id,
        completed.version,
        TaskRepositoryMergeBatchState::Blocked,
        &now_rfc3339(),
    )
    .await
    .expect_err("completed batch cannot return to a live state");
    assert!(matches!(
        backwards,
        DbError::Check(_) | DbError::InvalidTransition
    ));
    let child_backwards = sqlx::query(
        "UPDATE task_repository_merge_batch_repo SET state = 'pending'
         WHERE batch_id = ? AND repo_id = ?",
    )
    .bind(&batch_id)
    .bind(&repo_id)
    .execute(db.pool())
    .await;
    assert!(
        child_backwards.is_err(),
        "repository state cannot move backwards"
    );
}

#[tokio::test]
async fn multirepo_integration_single_repo_task_repository_migration_regression() {
    let db = sqlite_db().await;
    let project_id = seed_project(&db, "M3 single repo", None).await;
    let repo_id = multirepo_seed_repo(&db, &project_id, "single-primary").await;
    let task_id = multirepo_seed_task(
        &db,
        &project_id,
        &repo_id,
        vec![TaskRepositoryWrite {
            repo_id: repo_id.clone(),
            role: TaskRepositoryRole::Primary,
        }],
    )
    .await;
    let now = now_rfc3339();
    let workspace = WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: task_id.clone(),
            repo_id: repo_id.clone(),
            worktree_path: "/tmp/forge-m3-single-repo".to_owned(),
            branch: "integration/main".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: Some("base-sha".to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("single-repo integration Workspace creates");
    let current = TaskRepositorySetRepo::get_current(&db, &task_id)
        .await
        .expect("single-repo current revision loads")
        .expect("Workspace creation initializes the Task revision");
    assert_eq!(current.repositories.len(), 1);
    assert_eq!(current.repositories[0].repo_id, repo_id);
    let exact = WorkspaceRepo::get_integration_by_task_repo(&db, &task_id, &repo_id)
        .await
        .expect("single-repo exact integration lookup succeeds")
        .expect("single-repo Workspace exists");
    assert_eq!(exact, workspace);
    let listed = WorkspaceRepo::list_integrations_by_task(&db, &task_id)
        .await
        .expect("single-repo integration list succeeds");
    assert_eq!(listed, vec![workspace]);
    assert_foreign_key_check_is_clear(db.pool()).await;
}
