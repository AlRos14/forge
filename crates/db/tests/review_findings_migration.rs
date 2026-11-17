use db::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, run_migrations_from,
    CreateProject, CreateTask, ProjectRepo, SqliteDb, TaskCoordinationRepo,
    TaskCoordinationRequestKind, TaskLifecycleState, TaskRepo,
};
use std::{fs, path::Path};

#[tokio::test]
async fn v168_preserves_existing_request_history_and_reference_targets() {
    let migrations = tempfile::tempdir().expect("disposable migrations");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in fs::read_dir(source).expect("migrations directory") {
        let entry = entry.expect("migration entry");
        let name = entry.file_name();
        let name = name.to_str().expect("migration filename");
        let Some(version) = name
            .strip_prefix('V')
            .and_then(|name| name.split_once("__"))
            .and_then(|(version, _)| version.parse::<i64>().ok())
        else {
            continue;
        };
        if version <= 167 {
            fs::copy(entry.path(), migrations.path().join(name)).expect("copy baseline");
        }
    }
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("V167 baseline");
    let db = SqliteDb::new(pool.clone());
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    ProjectRepo::create(
        &db,
        CreateProject {
            id: project_id.clone(),
            name: "Disposable V168 preservation fixture".to_owned(),
            settings: "{}".to_owned(),
            primary_repo_id: None,
            owner_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("project");
    TaskRepo::create(
        &db,
        CreateTask {
            id: task_id.clone(),
            project_id,
            repo_id: None,
            parent_task_id: None,
            subtask_order: None,
            title: "Historical coordination requests".to_owned(),
            description: None,
            lifecycle_state: TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            merge_config: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("task");
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
            "INSERT INTO task_coordination_request(id,task_id,authorization_version,mode,
             brief_json,objective_digest,request_digest,idempotency_key,state,version,created_at,updated_at)
             VALUES(?,?,1,'prepare_and_execute','{}',?,?,?, ?,7,?,?)",
        )
        .bind(&id)
        .bind(&task_id)
        .bind("a".repeat(64))
        .bind("b".repeat(64))
        .bind(format!("historical-{state}"))
        .bind(state)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical request");
        ids.push((id, state));
    }
    run_migrations(&pool).await.expect("V168 upgrade");
    run_migrations(&pool).await.expect("idempotent upgrade");
    for (id, state) in ids {
        let request = TaskCoordinationRepo::get_task_coordination_request(&db, &id)
            .await
            .expect("request query")
            .expect("preserved request");
        assert_eq!(request.kind, TaskCoordinationRequestKind::WorkUnitPlan);
        assert_eq!(request.state.to_string(), state);
        assert_eq!(request.version, 7);
        assert_eq!(request.created_at, now);
        assert_eq!(request.objective_digest, "a".repeat(64));
        assert_eq!(request.request_digest, "b".repeat(64));
        assert!(request.review_report_artifact_id.is_none());
        assert!(request.review_report_digest.is_none());
        assert!(request.review_producer_execution_id.is_none());
        assert!(
            sqlx::query("UPDATE task_coordination_request SET brief_json='{} ' WHERE id=?")
                .bind(&id)
                .execute(&pool)
                .await
                .is_err(),
            "historical source cannot change after migration"
        );
    }
    let bad_references: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sqlite_schema WHERE sql LIKE '%task_coordination_request_v158%'
         OR sql LIKE '%project_plan_batch_coordination_request_v167%'",
    )
    .fetch_one(&pool)
    .await
    .expect("reference audit");
    assert_eq!(
        bad_references, 0,
        "no dependent schema object is retargeted"
    );
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
