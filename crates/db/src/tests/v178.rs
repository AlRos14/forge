//! V178 narrowly admits the workspace-free coordinator Execution that triages
//! an external-feedback request while its Task remains Merging.

use super::{seed_user, sqlite_db};
use crate::{new_uuid_v4, now_rfc3339, SqliteDb};

async fn merging_task(db: &SqliteDb, owner_id: &str) -> String {
    let now = now_rfc3339();
    let project_id = new_uuid_v4();
    let repo_id = new_uuid_v4();
    let task_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO project (id, name, owner_id, created_at, updated_at)
         VALUES (?, 'v178', ?, ?, ?)",
    )
    .bind(&project_id)
    .bind(owner_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("project");
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, work_mode, created_at, updated_at)
         VALUES (?, ?, 'v178', 'https://example.invalid/v178.git', 'direct_merge', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("repo");

    // This test isolates the V178 execution admission rule and seeds the
    // authoritative lifecycle row directly.
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at)
         VALUES (?, ?, ?, 'v178', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("task");
    sqlx::query(
        "INSERT INTO task_lifecycle (
            task_id, state, version, reason_kind, reason_ref, created_at, updated_at
         ) VALUES (?, 'merging', 1, 'task_merge', 'v178-fixture', ?, ?)",
    )
    .bind(&task_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("Task lifecycle in Merging state");
    task_id
}

async fn feedback_request(
    db: &SqliteDb,
    task_id: &str,
    id: &str,
    authorization_version: i64,
    state: &str,
    execution_id: Option<&str>,
) {
    let now = now_rfc3339();
    let digest = "a".repeat(64);
    sqlx::query(
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json,
            objective_digest, request_digest, idempotency_key, state, execution_id,
            created_at, updated_at, kind, source_subject_kind, source_subject_id,
            source_subject_digest
         ) VALUES (?, ?, ?, 'prepare_and_execute', '{}', ?, ?, ?, ?, ?, ?, ?,
                   'external_feedback', 'external_feedback_set', ?, ?)",
    )
    .bind(id)
    .bind(task_id)
    .bind(authorization_version)
    .bind(&digest)
    .bind(&digest)
    .bind(format!("v178:{id}"))
    .bind(state)
    .bind(execution_id)
    .bind(&now)
    .bind(&now)
    .bind(format!("admission:{id}"))
    .bind(&digest)
    .execute(db.pool())
    .await
    .expect("external feedback coordination request");
}

async fn insert_execution(
    db: &SqliteDb,
    task_id: &str,
    actor_id: &str,
    id: &str,
    status: &str,
    work_unit_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO execution (
            id, task_id, role, status, actor_kind, actor_id,
            work_unit_id, work_unit_version, created_at, updated_at
         ) VALUES (?, ?, 'executor', ?, 'human', ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(task_id)
    .bind(status)
    .bind(actor_id)
    .bind(work_unit_id)
    .bind(work_unit_id.map(|_| 1_i64))
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .map(|_| ())
}

fn assert_v178_rejection(error: sqlx::Error) {
    let message = match error {
        sqlx::Error::Database(error) => error.message().to_owned(),
        other => other.to_string(),
    };
    assert_eq!(
        message,
        "Running Execution admission requires a non-terminal Task lifecycle"
    );
}

#[tokio::test]
async fn v178_admits_only_unallocated_running_feedback_triage_in_merging() {
    let db = sqlite_db().await;
    let user_id = seed_user(&db).await;
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(db.pool())
        .await
        .expect("foreign keys off for linked request fixture");
    // The two external-feedback source guards need a real admission and
    // coordination authorization, which this focused trigger test does not
    // model. Drop only those guards and the unrelated WorkUnit admission guard.
    for trigger in [
        "task_coordination_request_subject_source_guard",
        "task_coordination_request_external_feedback_source_guard",
        "work_unit_execution_admission_guard",
    ] {
        sqlx::query(&format!("DROP TRIGGER {trigger}"))
            .execute(db.pool())
            .await
            .expect("drop unrelated authority guard for fixture setup");
    }

    // Requested and unlinked is the normal admission point for a coordinator
    // turn; a request already linked to this exact Execution is also valid.
    let task_id = merging_task(&db, &user_id).await;
    feedback_request(&db, &task_id, "request-unlinked", 1, "requested", None).await;
    insert_execution(
        &db,
        &task_id,
        &user_id,
        "execution-unlinked",
        "running",
        None,
    )
    .await
    .expect("unlinked external-feedback request admits its Running Execution");

    let task_id = merging_task(&db, &user_id).await;
    feedback_request(
        &db,
        &task_id,
        "request-linked",
        2,
        "uncertain",
        Some("execution-linked"),
    )
    .await;
    insert_execution(&db, &task_id, &user_id, "execution-linked", "running", None)
        .await
        .expect("request linked to this exact Execution is admissible");

    let task_id = merging_task(&db, &user_id).await;
    insert_execution(&db, &task_id, &user_id, "execution-other", "failed", None)
        .await
        .expect("the mismatching request names a real, unrelated Execution");
    feedback_request(
        &db,
        &task_id,
        "request-wrong-execution",
        3,
        "running",
        Some("execution-other"),
    )
    .await;
    assert_v178_rejection(
        insert_execution(
            &db,
            &task_id,
            &user_id,
            "execution-wrong-link",
            "running",
            None,
        )
        .await
        .expect_err("a request linked to another Execution is not authority"),
    );

    let task_id = merging_task(&db, &user_id).await;
    feedback_request(&db, &task_id, "request-terminal", 4, "failed", None).await;
    assert_v178_rejection(
        insert_execution(
            &db,
            &task_id,
            &user_id,
            "execution-terminal-request",
            "running",
            None,
        )
        .await
        .expect_err("terminal feedback requests do not admit Executions"),
    );

    let task_id = merging_task(&db, &user_id).await;
    feedback_request(&db, &task_id, "request-work-unit", 5, "requested", None).await;
    assert_v178_rejection(
        insert_execution(
            &db,
            &task_id,
            &user_id,
            "execution-work-unit",
            "running",
            Some("work-unit-not-permitted"),
        )
        .await
        .expect_err("external-feedback triage cannot admit a WorkUnit Execution"),
    );

    // The recreated UPDATE guard enforces the same predicate as INSERT.
    let task_id = merging_task(&db, &user_id).await;
    insert_execution(
        &db,
        &task_id,
        &user_id,
        "execution-update-valid",
        "failed",
        None,
    )
    .await
    .expect("non-running Execution may be recorded on Merging Task");
    feedback_request(
        &db,
        &task_id,
        "request-update-valid",
        6,
        "running",
        Some("execution-update-valid"),
    )
    .await;
    sqlx::query("UPDATE execution SET status = 'running' WHERE id = 'execution-update-valid'")
        .execute(db.pool())
        .await
        .expect("matching request admits a Running status update");

    let task_id = merging_task(&db, &user_id).await;
    insert_execution(
        &db,
        &task_id,
        &user_id,
        "execution-update-invalid",
        "failed",
        None,
    )
    .await
    .expect("non-running Execution may be recorded without a request");
    let error = sqlx::query(
        "UPDATE execution SET status = 'running' WHERE id = 'execution-update-invalid'",
    )
    .execute(db.pool())
    .await
    .expect_err("an unrelated status update cannot start on a Merging Task");
    assert_v178_rejection(error);
}
