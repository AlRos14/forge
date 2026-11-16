//! Phase 2 persistence of the coordinator-verification-and-pr-loop plan (PX,
//! D3'): V175 non-destructive PR admission pause for same-PR rework and the
//! per-item feedback dispositions.

use super::{copy_migrations_through, seed_user, sqlite_db};
use crate::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, run_migrations_from,
    CreateDomainEvent, CreateExternalFeedbackDisposition, CreateExternalFeedbackItem,
    CreateExternalFeedbackTriage, DbError, DomainEventRepo, ExternalFeedbackDisposition,
    ExternalFeedbackHeadState, ExternalFeedbackKind, ExternalFeedbackRepo,
    ExternalFeedbackSetEntry, ExternalFeedbackTrustClass, FeedbackDismissReason,
    FeedbackItemDisposition, FeedbackScopeClassification, PrReworkRepo, PrReworkState, SqliteDb,
    SupersedeAdmission, TaskLifecycleRepo, TaskLifecycleState, TransitionPrReworkSupersession,
    TransitionTaskLifecycle,
};
use sha2::Digest as _;

fn digest64(seed: &str) -> String {
    hex::encode(sha2::Sha256::digest(seed.as_bytes()))
}

fn policy_json() -> String {
    serde_json::json!({
        "schema_version": 1,
        "ingest": true,
        "trusted_author_associations": ["OWNER"],
        "trusted_logins": [],
        "max_items_per_poll": 50,
        "push_corrections": "automatic",
        "post_summary": "automatic",
    })
    .to_string()
}

const HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct World {
    db: SqliteDb,
    user: String,
    project: String,
    repo: String,
    task: String,
    cfg: String,
    meta: String,
    merge: String,
    publish: String,
    request: String,
    execution: String,
    gate_eval: String,
    items: Vec<(String, String)>,
}

/// Execute `sql` binding every argument as text (SQLite affinity converts).
async fn run(db: &SqliteDb, sql: &str, args: &[&str]) -> Result<(), sqlx::Error> {
    let mut query = sqlx::query(sql);
    for arg in args {
        query = query.bind(*arg);
    }
    query.execute(db.pool()).await.map(|_| ())
}

async fn ok(db: &SqliteDb, sql: &str, args: &[&str]) {
    run(db, sql, args)
        .await
        .unwrap_or_else(|error| panic!("expected success for `{sql}`: {error}"));
}

async fn rejected(db: &SqliteDb, sql: &str, args: &[&str]) {
    assert!(
        run(db, sql, args).await.is_err(),
        "expected rejection for `{sql}`"
    );
}

async fn seed_ops(w: &World, merge: &str, publish: &str) {
    let now = now_rfc3339();
    ok(
        &w.db,
        "INSERT INTO task_integration_operation (id, task_id, kind, owner_id, status, version,
            created_at, updated_at, gate_evaluation_id, remote_waiting)
         VALUES (?, ?, 'task_merge', 'owner', 'running', 1, ?, ?, ?, 1)",
        &[merge, &w.task, &now, &now, &w.gate_eval],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO task_integration_operation (id, task_id, kind, owner_id, status, version,
            created_at, updated_at, gate_evaluation_id, remote_waiting, parent_operation_id)
         VALUES (?, ?, 'publish_pr', 'owner', 'running', 1, ?, ?, ?, 0, ?)",
        &[publish, &w.task, &now, &now, &w.gate_eval, merge],
    )
    .await;
}

async fn admit(
    w: &World,
    merge: &str,
    publish: &str,
    sha: &str,
    readmits: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = now_rfc3339();
    let policy = policy_json();
    let digest = crate::pr_feedback_policy_digest(&policy);
    let provider_config_revision: String =
        sqlx::query_scalar("SELECT updated_at FROM pr_provider_config WHERE id = ?")
            .bind(&w.cfg)
            .fetch_one(w.db.pool())
            .await?;
    sqlx::query(
        "INSERT INTO remote_pr_admission (
            task_merge_operation_id, publish_operation_id, metadata_id, task_id,
            provider_config_id, provider_type, provider_config_revision,
            provider_config_digest, remote_repo_identity, source_branch, target_branch,
            admitted_source_sha, state, created_at, updated_at, feedback_policy_json,
            feedback_policy_digest, readmits_supersession_id
         ) VALUES (?, ?, ?, ?, ?, 'github', ?, 'digest', 'https://example.invalid/r.git',
                   'task/x', 'main', ?, 'admitted', ?, ?, ?, ?, ?)",
    )
    .bind(merge)
    .bind(publish)
    .bind(&w.meta)
    .bind(&w.task)
    .bind(&w.cfg)
    .bind(&provider_config_revision)
    .bind(sha)
    .bind(&now)
    .bind(&now)
    .bind(&policy)
    .bind(&digest)
    .bind(readmits)
    .execute(w.db.pool())
    .await
    .map(|_| ())
}

/// A Task in `merging` owning one `open` admission (PR `pr-77`) and a request
/// `external_feedback` over two trusted items, still `running`.
async fn world() -> World {
    let db = sqlite_db().await;
    run(&db, "PRAGMA foreign_keys = OFF", &[])
        .await
        .expect("fk off");
    for trigger in [
        "task_merge_gate_admission_guard",
        "task_publish_pr_gate_admission_guard",
        "pr9_task_merge_current_inputs_guard",
    ] {
        run(&db, &format!("DROP TRIGGER IF EXISTS {trigger}"), &[])
            .await
            .expect("drop unrelated gate guard");
    }
    let now = now_rfc3339();
    let user = seed_user(&db).await;
    let w = World {
        user,
        project: new_uuid_v4(),
        repo: new_uuid_v4(),
        task: new_uuid_v4(),
        cfg: new_uuid_v4(),
        meta: new_uuid_v4(),
        merge: new_uuid_v4(),
        publish: new_uuid_v4(),
        request: new_uuid_v4(),
        execution: new_uuid_v4(),
        gate_eval: new_uuid_v4(),
        items: Vec::new(),
        db,
    };
    ok(
        &w.db,
        "INSERT INTO project (id, name, owner_id, created_at, updated_at) VALUES (?, 'p', ?, ?, ?)",
        &[&w.project, &w.user, &now, &now],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO repo (id, project_id, name, remote_url, work_mode, created_at, updated_at)
         VALUES (?, ?, 'r', 'https://example.invalid/r.git', 'pull_request', ?, ?)",
        &[&w.repo, &w.project, &now, &now],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO task (id, project_id, repo_id, title, created_at, updated_at)
         VALUES (?, ?, ?, 't', ?, ?)",
        &[&w.task, &w.project, &w.repo, &now, &now],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO pr_provider_config (id, repo_id, provider_type, feedback_policy_json,
            created_at, updated_at) VALUES (?, ?, 'github', ?, ?, ?)",
        &[&w.cfg, &w.repo, &policy_json(), &now, &now],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO task_lifecycle (task_id, state, version, reason_kind, reason_ref,
            created_at, updated_at) VALUES (?, 'merging', 3, 'task_merge', ?, ?, ?)",
        &[&w.task, &w.merge, &now, &now],
    )
    .await;
    seed_ops(&w, &w.merge, &w.publish).await;
    ok(
        &w.db,
        "INSERT INTO pr_metadata (id, task_id, provider_type, provider_pr_id, pr_url,
            source_branch, target_branch, pr_state, merge_status, created_at, updated_at,
            task_merge_operation_id, publish_operation_id)
         VALUES (?, ?, 'github', 'pr-77', 'https://example.invalid/pull/77', 'task/x', 'main',
                 'open', 'pending', ?, ?, ?, ?)",
        &[&w.meta, &w.task, &now, &now, &w.merge, &w.publish],
    )
    .await;
    admit(&w, &w.merge, &w.publish, HEAD, None)
        .await
        .expect("admission");
    ok(
        &w.db,
        "UPDATE remote_pr_admission SET state = 'open', provider_status = 'open',
            observed_head_sha = ? WHERE task_merge_operation_id = ?",
        &[HEAD, &w.merge],
    )
    .await;
    ok(
        &w.db,
        "UPDATE pr_metadata SET admission_status = 'open' WHERE id = ?",
        &[&w.meta],
    )
    .await;
    ok(
        &w.db,
        "UPDATE task_integration_operation SET status = 'succeeded', finished_at = ?, version = 2
         WHERE id = ?",
        &[&now, &w.publish],
    )
    .await;
    w
}

/// Authorize coordination, create the `external_feedback` request over two
/// trusted items and start it (state `running`, bound to `w.execution`).
async fn start_request(w: &mut World) {
    let now = now_rfc3339();
    let role = new_uuid_v4();
    ok(
        &w.db,
        "INSERT INTO task_role (id, task_id, role, policy_json, created_at, updated_at)
         VALUES (?, ?, 'coordinator', '{}', ?, ?)",
        &[&role, &w.task, &now, &now],
    )
    .await;
    ok(
        &w.db,
        "UPDATE task_coordination SET mode = 'prepare_and_execute', coordinator_task_role_id = ?,
            coordinator_actor_kind = 'agent', coordinator_actor_id = 'agent-v175',
            authorization_version = 1, envelope_json = '{\"schema_version\":2}',
            envelope_digest = ?, team_digest = ?, requested_by = ? WHERE task_id = ?",
        &[
            &role,
            &digest64("envelope"),
            &digest64("team"),
            &w.user,
            &w.task,
        ],
    )
    .await;
    ok(
        &w.db,
        "UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?",
        &[&w.task],
    )
    .await;
    let mut ingested = Vec::new();
    for index in 0..2 {
        let body = format!("body{index}");
        ingested.push(CreateExternalFeedbackItem {
            id: new_uuid_v4(),
            task_id: w.task.clone(),
            admission_id: w.merge.clone(),
            provider_item_id: format!("c{index}"),
            kind: ExternalFeedbackKind::ReviewComment,
            author_login: "reviewer".to_owned(),
            author_association: "OWNER".to_owned(),
            review_state: None,
            path: Some("src/lib.rs".to_owned()),
            line: Some(3),
            commit_sha: Some(HEAD.to_owned()),
            body: body.clone(),
            body_truncated: false,
            body_digest: digest64(&body),
            provider_url: None,
            trust_class: ExternalFeedbackTrustClass::Trusted,
            head_state: ExternalFeedbackHeadState::Current,
            ingested_at: now_rfc3339(),
        });
    }
    let stored = ExternalFeedbackRepo::ingest_external_feedback(&w.db, ingested)
        .await
        .expect("ingest");
    w.items = stored
        .into_iter()
        .map(|(item, _)| (item.id, item.body_digest))
        .collect();
    ok(
        &w.db,
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json, objective_digest,
            request_digest, idempotency_key, state, version, created_at, updated_at,
            kind, source_subject_kind, source_subject_id, source_subject_digest
         ) VALUES (?, ?, 1, 'prepare_and_execute', '{}', ?, ?, ?, 'requested', 1, ?, ?,
                   'external_feedback', 'external_feedback_set', ?, ?)",
        &[
            &w.request,
            &w.task,
            &digest64("objective"),
            &digest64("request"),
            &format!("key-{}", w.request),
            &now,
            &now,
            &w.merge,
            &digest64("set"),
        ],
    )
    .await;
    let entries = w
        .items
        .iter()
        .map(|(id, digest)| ExternalFeedbackSetEntry {
            feedback_item_id: id.clone(),
            body_digest: digest.clone(),
        })
        .collect::<Vec<_>>();
    ExternalFeedbackRepo::add_external_feedback_set_items(&w.db, &w.request, &w.task, &entries)
        .await
        .expect("set items");
    ok(
        &w.db,
        "INSERT INTO execution (id, task_id, status, created_at, updated_at)
         VALUES (?, ?, 'completed', ?, ?)",
        &[&w.execution, &w.task, &now, &now],
    )
    .await;
    ok(
        &w.db,
        "UPDATE task_coordination_request SET state = 'running', version = 2, execution_id = ?
         WHERE id = ?",
        &[&w.execution, &w.request],
    )
    .await;
}

/// Record both triage rows and dispositions (`extend_task` for item 0 unless
/// `extend` is false, `dismiss` for item 1) and move the request to `triaged`.
async fn triage_all(w: &World, extend: bool) {
    for (index, (item_id, digest)) in w.items.iter().enumerate() {
        let disposition = if index == 0 && extend {
            FeedbackItemDisposition::ExtendTask
        } else {
            FeedbackItemDisposition::Dismiss
        };
        let triage = ExternalFeedbackRepo::record_external_feedback_triage(
            &w.db,
            CreateExternalFeedbackTriage {
                id: new_uuid_v4(),
                task_id: w.task.clone(),
                request_id: w.request.clone(),
                feedback_item_id: item_id.clone(),
                body_digest: digest.clone(),
                disposition: disposition.legacy(),
                rationale: "because".to_owned(),
                instructions: "do it".to_owned(),
                decision_digest: digest64(item_id),
                execution_id: w.execution.clone(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("triage row");
        ExternalFeedbackRepo::record_external_feedback_disposition(
            &w.db,
            CreateExternalFeedbackDisposition {
                id: new_uuid_v4(),
                task_id: w.task.clone(),
                triage_id: triage.id,
                request_id: w.request.clone(),
                feedback_item_id: item_id.clone(),
                body_digest: digest.clone(),
                disposition,
                scope_classification: (disposition == FeedbackItemDisposition::ExtendTask)
                    .then_some(FeedbackScopeClassification::WithinTaskScope),
                classification_facts_json: None,
                dismiss_reason: (disposition == FeedbackItemDisposition::Dismiss)
                    .then_some(FeedbackDismissReason::Disagree),
                proposal_json: None,
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("disposition");
    }
    ok(
        &w.db,
        "UPDATE task_coordination_request SET state = 'triaged', version = 3 WHERE id = ?",
        &[&w.request],
    )
    .await;
}

fn supersede(w: &World, id: &str) -> SupersedeAdmission {
    SupersedeAdmission {
        id: id.to_owned(),
        task_merge_operation_id: w.merge.clone(),
        request_id: w.request.clone(),
        now: now_rfc3339(),
    }
}

async fn lifecycle(w: &World) -> crate::TaskLifecycle {
    TaskLifecycleRepo::get_task_lifecycle(&w.db, &w.task)
        .await
        .expect("lifecycle")
        .expect("lifecycle exists")
}

async fn paused_world() -> (World, String) {
    let mut w = world().await;
    start_request(&mut w).await;
    triage_all(&w, true).await;
    let id = new_uuid_v4();
    PrReworkRepo::create_supersession(&w.db, supersede(&w, &id))
        .await
        .expect("supersede");
    (w, id)
}

#[tokio::test]
async fn v175_migration_applies_after_v174_and_only_recreates_listed_triggers() {
    let migrations = tempfile::tempdir().expect("migrations");
    copy_migrations_through(174, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("baseline V001-V174");
    let before: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master ORDER BY type, name")
            .fetch_all(&pool)
            .await
            .expect("objects before");
    let table_sql = |name: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT sql FROM sqlite_master WHERE name = ?")
                .bind(name)
                .fetch_one(&pool)
                .await
                .expect("table sql")
        }
    };
    let rebuilt_guard = [
        "remote_pr_history",
        "pr_metadata",
        "task_integration_operation",
        "task_external_feedback_triage",
    ];
    let mut tables_before = Vec::new();
    for name in rebuilt_guard {
        tables_before.push(table_sql(name).await);
    }
    let admission_before = table_sql("remote_pr_admission").await;

    run_migrations(&pool).await.expect("V175 applies");

    let after: Vec<String> =
        sqlx::query_scalar("SELECT name FROM sqlite_master ORDER BY type, name")
            .fetch_all(&pool)
            .await
            .expect("objects after");
    let missing: Vec<&String> = before.iter().filter(|name| !after.contains(name)).collect();
    assert!(
        missing.is_empty(),
        "V175 must not drop objects: {missing:?}"
    );
    for added in [
        "pr_rework_supersession",
        "task_external_feedback_disposition",
        "idx_pr_rework_supersession_open",
        "pr_rework_supersession_insert_guard",
        "pr_rework_supersession_update_guard",
        "remote_pr_admission_rework_guard",
        "remote_pr_admission_superseded_immutable",
        "task_lifecycle_pr_feedback_rework_guard",
        "task_external_feedback_disposition_insert_guard",
    ] {
        assert!(after.iter().any(|name| name == added), "{added} is created");
    }
    // Additive only: these tables are untouched and remote_pr_admission only
    // gained its three columns (never rebuilt).
    for (name, before) in rebuilt_guard.iter().zip(&tables_before) {
        assert_eq!(&table_sql(name).await, before, "{name} was rebuilt");
    }
    let admission_after = table_sql("remote_pr_admission").await;
    assert_ne!(admission_after, admission_before);
    for column in [
        "rework_outcome",
        "supersession_id",
        "readmits_supersession_id",
    ] {
        assert!(admission_after.contains(column), "{column} is added");
    }
    let integrity: String = sqlx::query_scalar("PRAGMA integrity_check")
        .fetch_one(&pool)
        .await
        .expect("integrity");
    assert_eq!(integrity, "ok");
    let violations = sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("fk check");
    assert!(violations.is_empty());
}

#[tokio::test]
async fn v175_dispositions_map_to_the_legacy_triage_and_are_immutable() {
    let mut w = world().await;
    start_request(&mut w).await;
    triage_all(&w, true).await;
    let rows = ExternalFeedbackRepo::list_external_feedback_dispositions(&w.db, &w.request)
        .await
        .expect("dispositions");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].disposition, FeedbackItemDisposition::ExtendTask);
    assert_eq!(
        rows[0].legacy_disposition,
        ExternalFeedbackDisposition::Correct
    );
    assert_eq!(rows[1].disposition, FeedbackItemDisposition::Dismiss);
    assert_eq!(
        rows[1].dismiss_reason,
        Some(FeedbackDismissReason::Disagree)
    );

    // Replay is a no-op; a different disposition for the same item conflicts.
    let triage = ExternalFeedbackRepo::list_external_feedback_triage(&w.db, &w.request)
        .await
        .expect("triage");
    let replay = ExternalFeedbackRepo::record_external_feedback_disposition(
        &w.db,
        CreateExternalFeedbackDisposition {
            id: new_uuid_v4(),
            task_id: w.task.clone(),
            triage_id: triage[0].id.clone(),
            request_id: w.request.clone(),
            feedback_item_id: w.items[0].0.clone(),
            body_digest: w.items[0].1.clone(),
            disposition: FeedbackItemDisposition::ExtendTask,
            scope_classification: Some(FeedbackScopeClassification::WithinTaskScope),
            classification_facts_json: None,
            dismiss_reason: None,
            proposal_json: None,
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("identical replay");
    assert_eq!(replay.id, rows[0].id);
    let conflict = ExternalFeedbackRepo::record_external_feedback_disposition(
        &w.db,
        CreateExternalFeedbackDisposition {
            id: new_uuid_v4(),
            task_id: w.task.clone(),
            triage_id: triage[0].id.clone(),
            request_id: w.request.clone(),
            feedback_item_id: w.items[0].0.clone(),
            body_digest: w.items[0].1.clone(),
            disposition: FeedbackItemDisposition::Dismiss,
            scope_classification: None,
            classification_facts_json: None,
            dismiss_reason: Some(FeedbackDismissReason::Duplicate),
            proposal_json: None,
            created_at: now_rfc3339(),
        },
    )
    .await;
    assert!(matches!(conflict, Err(DbError::IdempotencyConflict)));

    // CHECK: extend_task needs a within-scope classification.
    rejected(
        &w.db,
        "INSERT INTO task_external_feedback_disposition (id, task_id, triage_id, request_id,
            feedback_item_id, body_digest, disposition, legacy_disposition,
            scope_classification, created_at)
         VALUES ('bad', ?, ?, ?, ?, ?, 'extend_task', 'correct', 'scope_change', ?)",
        &[
            &w.task,
            &triage[1].id,
            &w.request,
            &w.items[1].0,
            &w.items[1].1,
            &now_rfc3339(),
        ],
    )
    .await;
    // Immutable except the set-once decision/follow-up links; no delete.
    rejected(
        &w.db,
        "UPDATE task_external_feedback_disposition SET dismiss_reason = 'duplicate'",
        &[],
    )
    .await;
    rejected(
        &w.db,
        "UPDATE task_external_feedback_disposition SET human_decision_id = 'x'
         WHERE disposition = 'extend_task'",
        &[],
    )
    .await;
    rejected(&w.db, "DELETE FROM task_external_feedback_disposition", &[]).await;
}

#[tokio::test]
async fn v175_supersession_is_atomic_exact_and_idempotent() {
    let mut w = world().await;
    start_request(&mut w).await;
    // A dismiss-only turn cannot pause the PR.
    triage_all(&w, false).await;
    let none = PrReworkRepo::create_supersession(&w.db, supersede(&w, &new_uuid_v4())).await;
    assert!(
        none.is_err(),
        "no extend_task disposition: nothing to rework"
    );
    assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Merging);

    let mut w = world().await;
    start_request(&mut w).await;
    triage_all(&w, true).await;
    let id = new_uuid_v4();
    let write = PrReworkRepo::create_supersession(&w.db, supersede(&w, &id))
        .await
        .expect("supersede");
    assert!(write.created);
    assert_eq!(write.events.len(), 2);
    assert_eq!(write.supersession.state, PrReworkState::Open);
    assert_eq!(write.supersession.provider_pr_id, "pr-77");
    assert_eq!(write.supersession.superseded_head_sha, HEAD);
    assert_eq!(write.supersession.triage_set_digest, digest64("set"));
    // TaskMerge abandoned, admission marked but still open, PR projection kept.
    let status: String =
        sqlx::query_scalar("SELECT status FROM task_integration_operation WHERE id = ?")
            .bind(&w.merge)
            .fetch_one(w.db.pool())
            .await
            .expect("merge status");
    assert_eq!(status, "abandoned");
    let (state, outcome, supersession): (String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT state, rework_outcome, supersession_id FROM remote_pr_admission
         WHERE task_merge_operation_id = ?",
    )
    .bind(&w.merge)
    .fetch_one(w.db.pool())
    .await
    .expect("admission");
    assert_eq!(state, "open");
    assert_eq!(outcome.as_deref(), Some("superseded_for_rework"));
    assert_eq!(supersession.as_deref(), Some(id.as_str()));
    let pr_state: String = sqlx::query_scalar("SELECT pr_state FROM pr_metadata WHERE id = ?")
        .bind(&w.meta)
        .fetch_one(w.db.pool())
        .await
        .expect("metadata");
    assert_eq!(pr_state, "open", "Forge never closes or touches the PR");
    let blocked = lifecycle(&w).await;
    assert_eq!(blocked.state, TaskLifecycleState::Blocked);
    assert_eq!(blocked.reason_kind.as_deref(), Some("pr_feedback_rework"));
    assert_eq!(blocked.reason_ref.as_deref(), Some(id.as_str()));
    // Replay returns the same receipt without new effects.
    let replay = PrReworkRepo::create_supersession(&w.db, supersede(&w, &new_uuid_v4()))
        .await
        .expect("replay");
    assert!(!replay.created);
    assert_eq!(replay.supersession.id, id);
    assert!(replay.events.is_empty());
    let by_task = PrReworkRepo::get_open_supersession_by_task(&w.db, &w.task)
        .await
        .expect("open by task")
        .expect("exists");
    assert_eq!(by_task.id, id);
    // The paused admission and receipt are immutable.
    rejected(
        &w.db,
        "UPDATE remote_pr_admission SET state = 'closed' WHERE task_merge_operation_id = ?",
        &[&w.merge],
    )
    .await;
    rejected(
        &w.db,
        "UPDATE remote_pr_admission SET rework_outcome = NULL WHERE task_merge_operation_id = ?",
        &[&w.merge],
    )
    .await;
    rejected(&w.db, "DELETE FROM pr_rework_supersession", &[]).await;
    rejected(
        &w.db,
        "UPDATE pr_rework_supersession SET provider_pr_id = 'other'",
        &[],
    )
    .await;
}

#[tokio::test]
async fn v175_supersession_requires_exact_receipts_and_waiting_task_merge() {
    let mut w = world().await;
    start_request(&mut w).await;
    triage_all(&w, true).await;
    let id = new_uuid_v4();
    let event_id = new_uuid_v4();
    let payload = |supersession: &str, head: &str| {
        serde_json::json!({
            "task_id": w.task, "pr_metadata_id": w.meta,
            "task_merge_operation_id": w.merge, "publish_operation_id": w.publish,
            "supersession_id": supersession, "provider_pr_id": "pr-77",
            "superseded_head_sha": head,
        })
        .to_string()
    };
    let event = |event_id: &str, event_type: &str, payload_json: String| CreateDomainEvent {
        id: event_id.to_owned(),
        event_type: event_type.to_owned(),
        entity_type: "pr_metadata".to_owned(),
        entity_id: w.meta.clone(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: w.task.clone(),
        correlation_id: "c".to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json,
        created_at: now_rfc3339(),
    };
    let insert = |sha: &str, result_event_id: &str| {
        let db = &w.db;
        let (id, request, task, merge, publish, meta, sha, result_event_id) = (
            id.clone(),
            w.request.clone(),
            w.task.clone(),
            w.merge.clone(),
            w.publish.clone(),
            w.meta.clone(),
            sha.to_owned(),
            result_event_id.to_owned(),
        );
        async move {
            run(
                db,
                "INSERT INTO pr_rework_supersession (id, task_id, task_merge_operation_id,
                    publish_operation_id, metadata_id, request_id, triage_set_digest,
                    provider_pr_id, superseded_head_sha, result_event_id, state, version,
                    created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 'pr-77', ?, ?, 'open', 1, ?, ?)",
                &[
                    &id,
                    &task,
                    &merge,
                    &publish,
                    &meta,
                    &request,
                    &digest64("set"),
                    &sha,
                    &result_event_id,
                    &now_rfc3339(),
                    &now_rfc3339(),
                ],
            )
            .await
        }
    };
    let wrong_event_id = new_uuid_v4();
    let mut tx = w.db.pool().begin().await.expect("tx");
    DomainEventRepo::append_event_in_tx(
        &w.db,
        &mut tx,
        &event(&wrong_event_id, "pr.status_changed", payload(&id, HEAD)),
    )
    .await
    .expect("wrong-type event");
    DomainEventRepo::append_event_in_tx(
        &w.db,
        &mut tx,
        &event(&event_id, "pr.admission_superseded", payload(&id, HEAD)),
    )
    .await
    .expect("exact event");
    tx.commit().await.expect("commit");
    assert!(
        insert(HEAD, &wrong_event_id).await.is_err(),
        "wrong event type"
    );
    assert!(
        insert("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb", &event_id)
            .await
            .is_err(),
        "the head must be the admitted source SHA"
    );
    insert(HEAD, &event_id).await.expect("exact receipt");
    // A running TaskMerge cannot be abandoned without the marked admission.
    rejected(
        &w.db,
        "UPDATE task_integration_operation SET status = 'abandoned', finished_at = ?,
            version = 2, result_event_id = ? WHERE id = ?",
        &[&now_rfc3339(), &event_id, &w.merge],
    )
    .await;
    rejected(
        &w.db,
        "UPDATE remote_pr_admission SET rework_outcome = 'superseded_for_rework',
            supersession_id = 'unknown' WHERE task_merge_operation_id = ?",
        &[&w.merge],
    )
    .await;
}

#[tokio::test]
async fn v175_lifecycle_pause_and_resume_receipts_are_exact() {
    let (w, id) = paused_world().await;
    let event_id: String =
        sqlx::query_scalar("SELECT result_event_id FROM pr_rework_supersession WHERE id = ?")
            .bind(&id)
            .fetch_one(w.db.pool())
            .await
            .expect("event");
    let blocked = lifecycle(&w).await;
    let task_version: i64 = sqlx::query_scalar("SELECT version FROM task WHERE id = ?")
        .bind(&w.task)
        .fetch_one(w.db.pool())
        .await
        .expect("task version");
    let resume = |cause_kind: &str, cause_ref: &str, to: TaskLifecycleState, reason_ref: &str| {
        let key = new_uuid_v4();
        TransitionTaskLifecycle {
            id: new_uuid_v4(),
            task_id: w.task.clone(),
            expected_task_version: task_version,
            expected_lifecycle_version: blocked.version,
            expected_state: TaskLifecycleState::Blocked,
            to_state: to,
            cause_kind: cause_kind.to_owned(),
            cause_ref: Some(cause_ref.to_owned()),
            reason_kind: Some("pr_feedback_rework".to_owned()),
            reason_ref: Some(reason_ref.to_owned()),
            gate_evaluation_id: None,
            idempotency_key: key.clone(),
            updated_at: now_rfc3339(),
            event: CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.lifecycle_changed".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: w.task.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: w.task.clone(),
                correlation_id: key,
                causation_id: None,
                causation_depth: 1,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now_rfc3339(),
            },
        }
    };
    // Wrong event, wrong cause kind, wrong target state, unknown supersession.
    for bad in [
        resume(
            "domain_event",
            &new_uuid_v4(),
            TaskLifecycleState::Active,
            &id,
        ),
        resume("merge_operation", &w.merge, TaskLifecycleState::Active, &id),
        resume("domain_event", &event_id, TaskLifecycleState::Ready, &id),
        resume(
            "domain_event",
            &event_id,
            TaskLifecycleState::Active,
            "unknown",
        ),
    ] {
        assert!(
            TaskLifecycleRepo::transition_task_lifecycle(&w.db, bad)
                .await
                .is_err(),
            "the receipt kind admits only its exact two edges"
        );
    }
    assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Blocked);
    TaskLifecycleRepo::transition_task_lifecycle(
        &w.db,
        resume("domain_event", &event_id, TaskLifecycleState::Active, &id),
    )
    .await
    .expect("exact Blocked -> Active receipt");
    let active = lifecycle(&w).await;
    assert_eq!(active.state, TaskLifecycleState::Active);
    assert_eq!(active.reason_kind.as_deref(), Some("pr_feedback_rework"));
}

#[tokio::test]
async fn v175_lifecycle_rejects_the_receipt_kind_on_other_edges_and_without_a_pause() {
    let mut w = world().await;
    start_request(&mut w).await;
    triage_all(&w, true).await;
    // Merging -> Blocked with the receipt kind but no supersession is rejected
    // even for an abandoned TaskMerge (V100 alone would accept it).
    let ghost = new_uuid_v4();
    ok(
        &w.db,
        "INSERT INTO task_integration_operation (id, task_id, kind, owner_id, status, version,
            created_at, updated_at, finished_at, gate_evaluation_id, remote_waiting)
         VALUES (?, ?, 'task_merge', 'owner', 'abandoned', 2, ?, ?, ?, ?, 1)",
        &[
            &ghost,
            &w.task,
            &now_rfc3339(),
            &now_rfc3339(),
            &now_rfc3339(),
            &w.gate_eval,
        ],
    )
    .await;
    let lifecycle_now = lifecycle(&w).await;
    let task_version: i64 = sqlx::query_scalar("SELECT version FROM task WHERE id = ?")
        .bind(&w.task)
        .fetch_one(w.db.pool())
        .await
        .expect("task version");
    let key = new_uuid_v4();
    let attempt = TransitionTaskLifecycle {
        id: new_uuid_v4(),
        task_id: w.task.clone(),
        expected_task_version: task_version,
        expected_lifecycle_version: lifecycle_now.version,
        expected_state: TaskLifecycleState::Merging,
        to_state: TaskLifecycleState::Blocked,
        cause_kind: "merge_operation".to_owned(),
        cause_ref: Some(ghost.clone()),
        reason_kind: Some("pr_feedback_rework".to_owned()),
        reason_ref: Some("not-a-supersession".to_owned()),
        gate_evaluation_id: None,
        idempotency_key: key.clone(),
        updated_at: now_rfc3339(),
        event: CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.lifecycle_changed".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: w.task.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: w.task.clone(),
            correlation_id: key,
            causation_id: None,
            causation_depth: 1,
            dedupe_key: None,
            payload_json: "{}".to_owned(),
            created_at: now_rfc3339(),
        },
    };
    assert!(TaskLifecycleRepo::transition_task_lifecycle(&w.db, attempt)
        .await
        .is_err());
    assert_eq!(lifecycle(&w).await.state, TaskLifecycleState::Merging);
}

#[tokio::test]
async fn v175_pause_closes_feedback_ingestion_and_new_requests_but_allows_comments() {
    let (w, _id) = paused_world().await;
    // Ingestion and new triage requests need an unpaused open admission.
    let late = CreateExternalFeedbackItem {
        id: new_uuid_v4(),
        task_id: w.task.clone(),
        admission_id: w.merge.clone(),
        provider_item_id: "late".to_owned(),
        kind: ExternalFeedbackKind::IssueComment,
        author_login: "reviewer".to_owned(),
        author_association: "OWNER".to_owned(),
        review_state: None,
        path: None,
        line: None,
        commit_sha: None,
        body: "late".to_owned(),
        body_truncated: false,
        body_digest: digest64("late"),
        provider_url: None,
        trust_class: ExternalFeedbackTrustClass::Trusted,
        head_state: ExternalFeedbackHeadState::Current,
        ingested_at: now_rfc3339(),
    };
    assert!(
        ExternalFeedbackRepo::ingest_external_feedback(&w.db, vec![late])
            .await
            .is_err()
    );
    rejected(
        &w.db,
        "INSERT INTO task_coordination_request (id, task_id, authorization_version, mode,
            brief_json, objective_digest, request_digest, idempotency_key, state, version,
            created_at, updated_at, kind, source_subject_kind, source_subject_id,
            source_subject_digest)
         VALUES ('r2', ?, 1, 'prepare_and_execute', '{}', ?, ?, 'k2', 'requested', 1, ?, ?,
                 'external_feedback', 'external_feedback_set', ?, ?)",
        &[
            &w.task,
            &digest64("objective"),
            &digest64("request"),
            &now_rfc3339(),
            &now_rfc3339(),
            &w.merge,
            &digest64("set"),
        ],
    )
    .await;
    // post_comment is allowed on the paused (still open) PR; push_update is not.
    let triggers: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'artifact'
           AND sql LIKE '%BEFORE INSERT%'",
    )
    .fetch_all(w.db.pool())
    .await
    .expect("artifact triggers");
    for name in triggers {
        run(&w.db, &format!("DROP TRIGGER {name}"), &[])
            .await
            .expect("drop");
    }
    ok(
        &w.db,
        "INSERT INTO artifact (id, task_id, kind, storage_kind, content, created_at)
         VALUES ('summary', ?, 'summary', 'inline', 'text', ?)",
        &[&w.task, &now_rfc3339()],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO pr_external_action (id, task_id, admission_id, kind, state,
            authorization_mode, summary_artifact_id, body_digest, idempotency_key, version,
            created_at, updated_at)
         VALUES ('comment', ?, ?, 'post_comment', 'approved', 'automatic', 'summary', ?, 'kc',
                 1, ?, ?)",
        &[
            &w.task,
            &w.merge,
            &digest64("x"),
            &now_rfc3339(),
            &now_rfc3339(),
        ],
    )
    .await;
    ok(
        &w.db,
        "UPDATE pr_external_action SET state = 'running', version = 2, updated_at = ?
         WHERE id = 'comment'",
        &[&now_rfc3339()],
    )
    .await;
    ok(
        &w.db,
        "UPDATE pr_external_action SET state = 'succeeded', version = 3, finished_at = ?,
            updated_at = ? WHERE id = 'comment'",
        &[&now_rfc3339(), &now_rfc3339()],
    )
    .await;
    ok(
        &w.db,
        "INSERT INTO gate_evaluation (id, gate_id, task_id, policy_revision, outcome,
            input_digest, result_json, evaluated_at)
         VALUES (?, 'g', ?, 1, 'satisfied', ?, '{}', ?)",
        &[&w.gate_eval, &w.task, &digest64("g"), &now_rfc3339()],
    )
    .await;
    rejected(
        &w.db,
        "INSERT INTO pr_external_action (id, task_id, admission_id, kind, state,
            authorization_mode, gate_evaluation_id, subject_commit_sha,
            expected_remote_head_sha, idempotency_key, version, created_at, updated_at)
         VALUES ('push', ?, ?, 'push_update', 'approved', 'automatic', ?, ?, ?, 'kp', 1, ?, ?)",
        &[
            &w.task,
            &w.merge,
            &w.gate_eval,
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            HEAD,
            &now_rfc3339(),
            &now_rfc3339(),
        ],
    )
    .await;
}

async fn move_to_new_merge(w: &World, merge: &str, publish: &str) {
    for trigger in [
        "task_lifecycle_update_guard",
        "task_lifecycle_transition_guard_insert",
    ] {
        run(&w.db, &format!("DROP TRIGGER IF EXISTS {trigger}"), &[])
            .await
            .expect("drop lifecycle guard");
    }
    ok(
        &w.db,
        "UPDATE task_lifecycle SET state = 'merging', version = 9,
            reason_kind = 'task_merge', reason_ref = ? WHERE task_id = ?",
        &[merge, &w.task],
    )
    .await;
    seed_ops(w, merge, publish).await;
}

#[tokio::test]
async fn v175_readmission_reuses_the_open_pr_and_must_name_the_pause() {
    let (w, id) = paused_world().await;
    let (merge2, publish2) = (new_uuid_v4(), new_uuid_v4());
    move_to_new_merge(&w, &merge2, &publish2).await;
    let sha2 = "cccccccccccccccccccccccccccccccccccccccc";
    // While a pause is open a fresh admission is rejected, and so is naming an
    // unknown pause.
    assert!(admit(&w, &merge2, &publish2, sha2, None).await.is_err());
    assert!(admit(&w, &merge2, &publish2, sha2, Some("unknown"))
        .await
        .is_err());
    // The metadata projection can rebind to the new TaskMerge keeping the PR.
    ok(
        &w.db,
        "UPDATE pr_metadata SET task_merge_operation_id = ?, publish_operation_id = ?,
            updated_at = ? WHERE id = ?",
        &[&merge2, &publish2, &now_rfc3339(), &w.meta],
    )
    .await;
    let pr_id: String = sqlx::query_scalar("SELECT provider_pr_id FROM pr_metadata WHERE id = ?")
        .bind(&w.meta)
        .fetch_one(w.db.pool())
        .await
        .expect("pr id");
    assert_eq!(pr_id, "pr-77", "the same PR stays bound");
    admit(&w, &merge2, &publish2, sha2, Some(&id))
        .await
        .expect("re-admission names the open pause");
    // The pause can only become `readmitted` with that exact admission.
    let current = PrReworkRepo::get_supersession(&w.db, &id)
        .await
        .expect("get")
        .expect("exists");
    let transition =
        |to_state, readmission: Option<&str>, version| TransitionPrReworkSupersession {
            id: id.clone(),
            expected_version: version,
            to_state,
            readmission_merge_operation_id: readmission.map(str::to_owned),
            convergence_decision_id: None,
            updated_at: now_rfc3339(),
        };
    assert!(PrReworkRepo::transition_supersession(
        &w.db,
        transition(
            PrReworkState::Readmitted,
            Some(w.merge.as_str()),
            current.version
        )
    )
    .await
    .is_err());
    assert!(PrReworkRepo::transition_supersession(
        &w.db,
        transition(
            PrReworkState::Readmitted,
            Some(&merge2),
            current.version + 5
        )
    )
    .await
    .is_err());
    let readmitted = PrReworkRepo::transition_supersession(
        &w.db,
        transition(
            PrReworkState::Readmitted,
            Some(merge2.as_str()),
            current.version,
        ),
    )
    .await
    .expect("readmitted");
    assert_eq!(readmitted.state, PrReworkState::Readmitted);
    assert_eq!(
        readmitted.readmission_merge_operation_id.as_deref(),
        Some(merge2.as_str())
    );
    // Terminal.
    assert!(PrReworkRepo::transition_supersession(
        &w.db,
        transition(PrReworkState::Abandoned, None, readmitted.version)
    )
    .await
    .is_err());
}

#[tokio::test]
async fn v175_remote_close_allows_a_new_pr_and_remote_merge_blocks_admission() {
    // Remote close: the next admission is fresh (no readmits) and may reset
    // the projection for a new PR.
    let (w, id) = paused_world().await;
    let current = PrReworkRepo::get_supersession(&w.db, &id)
        .await
        .expect("get")
        .expect("exists");
    PrReworkRepo::transition_supersession(
        &w.db,
        TransitionPrReworkSupersession {
            id: id.clone(),
            expected_version: current.version,
            to_state: PrReworkState::RemoteClosed,
            readmission_merge_operation_id: None,
            convergence_decision_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("remote_closed");
    let (merge2, publish2) = (new_uuid_v4(), new_uuid_v4());
    move_to_new_merge(&w, &merge2, &publish2).await;
    ok(
        &w.db,
        "UPDATE pr_metadata SET task_merge_operation_id = ?, publish_operation_id = ?,
            provider_pr_id = NULL, pr_url = NULL, pr_state = 'publishing',
            updated_at = ? WHERE id = ?",
        &[&merge2, &publish2, &now_rfc3339(), &w.meta],
    )
    .await;
    let sha2 = "cccccccccccccccccccccccccccccccccccccccc";
    assert!(admit(&w, &merge2, &publish2, sha2, Some(&id))
        .await
        .is_err());
    admit(&w, &merge2, &publish2, sha2, None)
        .await
        .expect("a fresh admission after the PR was closed remotely");

    // Remote merge: no admission until the convergence is abandoned.
    let (w, id) = paused_world().await;
    let current = PrReworkRepo::get_supersession(&w.db, &id)
        .await
        .expect("get")
        .expect("exists");
    let merged = PrReworkRepo::transition_supersession(
        &w.db,
        TransitionPrReworkSupersession {
            id: id.clone(),
            expected_version: current.version,
            to_state: PrReworkState::RemoteMerged,
            readmission_merge_operation_id: None,
            convergence_decision_id: Some("decision-1".to_owned()),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("remote_merged");
    let (merge2, publish2) = (new_uuid_v4(), new_uuid_v4());
    move_to_new_merge(&w, &merge2, &publish2).await;
    run(
        &w.db,
        "DROP TRIGGER IF EXISTS pr_metadata_merge_admission_guard_update",
        &[],
    )
    .await
    .expect("drop");
    ok(
        &w.db,
        "UPDATE pr_metadata SET task_merge_operation_id = ?, publish_operation_id = ? WHERE id = ?",
        &[&merge2, &publish2, &w.meta],
    )
    .await;
    assert!(admit(
        &w,
        &merge2,
        &publish2,
        "cccccccccccccccccccccccccccccccccccccccc",
        None
    )
    .await
    .is_err());
    PrReworkRepo::transition_supersession(
        &w.db,
        TransitionPrReworkSupersession {
            id,
            expected_version: merged.version,
            to_state: PrReworkState::Abandoned,
            readmission_merge_operation_id: None,
            convergence_decision_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("abandoned");
    admit(
        &w,
        &merge2,
        &publish2,
        "cccccccccccccccccccccccccccccccccccccccc",
        None,
    )
    .await
    .expect("admission after the merged pause was abandoned");
}

#[tokio::test]
async fn v175_provider_result_guard_keeps_legacy_rejections() {
    let w = world().await;
    rejected(
        &w.db,
        "UPDATE task_integration_operation SET status = 'failed', finished_at = ?, version = 2
         WHERE id = ?",
        &[&now_rfc3339(), &w.merge],
    )
    .await;
    rejected(
        &w.db,
        "UPDATE task_integration_operation SET status = 'abandoned', finished_at = ?, version = 2
         WHERE id = ?",
        &[&now_rfc3339(), &w.merge],
    )
    .await;
}
