//! Phase 2 persistence of the coordinator-verification-and-pr-loop plan (A3):
//! V173 GitHub credential reference, frozen feedback policy, external feedback
//! ledger/triage and PR external actions.

use super::{
    copy_migrations_through, page, seed_agent, seed_project, seed_task, seed_user, sqlite_db,
};
use crate::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, pr_feedback_policy_digest, run_migrations,
    run_migrations_from, CreateExternalFeedbackItem, CreateExternalFeedbackTriage,
    CreatePrExternalAction, CreatePrProviderConfig, CreateTaskRole, DbError,
    ExternalFeedbackDisposition, ExternalFeedbackHeadState, ExternalFeedbackKind,
    ExternalFeedbackRepo, ExternalFeedbackSetEntry, ExternalFeedbackTrustClass, PrExternalAction,
    PrExternalActionKind, PrExternalActionRepo, PrExternalActionState, PrFeedbackAuthorization,
    PrFeedbackPolicy, PrFeedbackSummaryMode, PrProviderConfigRepo, PrProviderCredentialRef,
    SqliteDb, TaskRoleRepo, TransitionPrExternalAction, DEFAULT_PR_FEEDBACK_POLICY_JSON,
};
use sha2::Digest as _;

fn digest64(seed: &str) -> String {
    hex::encode(sha2::Sha256::digest(seed.as_bytes()))
}

fn policy(ingest: bool, push: &str, post: &str) -> String {
    serde_json::json!({
        "schema_version": 1,
        "ingest": ingest,
        "trusted_author_associations": ["OWNER"],
        "trusted_logins": [],
        "max_items_per_poll": 50,
        "push_corrections": push,
        "post_summary": post,
    })
    .to_string()
}

async fn credential(db: &SqliteDb, owner: &str, provider: &str, status: &str) -> String {
    let id = new_uuid_v4();
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO credential_handle (id, owner_user_id, provider, label, status, created_at, updated_at)
         VALUES (?, ?, ?, 'label', ?, ?, ?)",
    )
    .bind(&id)
    .bind(owner)
    .bind(provider)
    .bind(status)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("credential handle");
    id
}

struct Fixture {
    db: SqliteDb,
    user_id: String,
    task_id: String,
    config_id: String,
}

async fn fixture() -> Fixture {
    let db = sqlite_db().await;
    let user_id = seed_user(&db).await;
    let project_id = seed_project(&db, "v173 fixture", Some(user_id.clone())).await;
    let now = now_rfc3339();
    let repo_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, created_at, updated_at)
         VALUES (?, ?, 'repo', 'https://example.invalid/repo.git', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("repo");
    let task_id = seed_task(
        &db,
        &project_id,
        &repo_id,
        None,
        "in_progress".to_owned(),
        "v173 task",
    )
    .await;
    let config = PrProviderConfigRepo::create(
        &db,
        CreatePrProviderConfig {
            id: new_uuid_v4(),
            repo_id: repo_id.clone(),
            provider_type: "github".to_owned(),
            base_url: None,
            polling_interval_seconds: 60,
            token_secret_ref: None,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("pr provider config");
    Fixture {
        db,
        user_id,
        task_id,
        config_id: config.id,
    }
}

/// Seed one `open` remote PR admission directly (the V112 insert guard needs a
/// full merge graph that is covered elsewhere; here its frozen columns matter).
async fn seed_admission(f: &Fixture, id: &str, state: &str, policy_json: Option<&str>) {
    let now = now_rfc3339();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(f.db.pool())
        .await
        .expect("fk off");
    let _ = sqlx::query("DROP TRIGGER IF EXISTS remote_pr_admission_insert_guard")
        .execute(f.db.pool())
        .await;
    sqlx::query(
        "INSERT INTO remote_pr_admission (
            task_merge_operation_id, publish_operation_id, metadata_id, task_id,
            provider_config_id, provider_type, provider_config_revision,
            provider_config_digest, remote_repo_identity, source_branch, target_branch,
            admitted_source_sha, state, feedback_policy_json, feedback_policy_digest,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, 'github', 'rev', 'digest', 'repo', 'src', 'main', ?, ?, ?, ?, ?, ?)",
    )
    .bind(id)
    .bind(format!("publish-{id}"))
    .bind(format!("meta-{id}"))
    .bind(&f.task_id)
    .bind(&f.config_id)
    .bind("a".repeat(40))
    .bind(state)
    .bind(policy_json)
    .bind(policy_json.map(pr_feedback_policy_digest))
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("admission");
}

fn feedback_input(
    f: &Fixture,
    admission_id: &str,
    provider_item_id: &str,
    body: &str,
) -> CreateExternalFeedbackItem {
    CreateExternalFeedbackItem {
        id: new_uuid_v4(),
        task_id: f.task_id.clone(),
        admission_id: admission_id.to_owned(),
        provider_item_id: provider_item_id.to_owned(),
        kind: ExternalFeedbackKind::ReviewComment,
        author_login: "reviewer".to_owned(),
        author_association: "OWNER".to_owned(),
        review_state: None,
        path: Some("src/lib.rs".to_owned()),
        line: Some(3),
        commit_sha: Some("a".repeat(40)),
        body: body.to_owned(),
        body_truncated: false,
        body_digest: digest64(body),
        provider_url: None,
        trust_class: ExternalFeedbackTrustClass::Trusted,
        head_state: ExternalFeedbackHeadState::Current,
        ingested_at: now_rfc3339(),
    }
}

#[tokio::test]
async fn v173_migration_applies_after_v174_and_preserves_pr_config_history() {
    // A database that already ran V174 but never saw V173 (the number was free).
    let migrations = tempfile::tempdir().expect("migrations");
    copy_migrations_through(172, migrations.path());
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    for entry in std::fs::read_dir(&source).expect("migration dir") {
        let path = entry.expect("entry").path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with("V174__") {
            std::fs::copy(&path, migrations.path().join(name)).expect("copy V174");
        }
    }
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("baseline V001-V172 + V174");
    let db = SqliteDb::new(pool.clone());
    let user_id = seed_user(&db).await;
    let project_id = seed_project(&db, "v173 legacy", Some(user_id)).await;
    let repo_id = new_uuid_v4();
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, created_at, updated_at)
         VALUES (?, ?, 'repo', 'https://example.invalid/repo.git', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("repo");
    sqlx::query(
        "INSERT INTO pr_provider_config (id, repo_id, provider_type, base_url,
            polling_interval_seconds, token_secret_ref, created_at, updated_at)
         VALUES ('legacy-config', ?, 'github', NULL, 120, 'GITHUB_TOKEN', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("legacy PR provider config");

    run_migrations(&pool)
        .await
        .expect("V173 applies out of order");

    let versions: Vec<i64> = sqlx::query_scalar(
        "SELECT version FROM _migration WHERE version IN (173, 174) ORDER BY version",
    )
    .fetch_all(&pool)
    .await
    .expect("receipts");
    assert_eq!(versions, vec![173, 174]);
    let config = PrProviderConfigRepo::get_by_repo_id(&db, &repo_id)
        .await
        .expect("config")
        .expect("legacy config survives");
    assert_eq!(config.token_secret_ref.as_deref(), Some("GITHUB_TOKEN"));
    assert_eq!(config.polling_interval_seconds, 120);
    assert_eq!(config.credential_handle_id, None);
    assert_eq!(config.credential_owner_user_id, None);
    assert_eq!(config.feedback_policy_json, DEFAULT_PR_FEEDBACK_POLICY_JSON);
    let default_policy = PrFeedbackPolicy::from_json(&config.feedback_policy_json).expect("policy");
    assert_eq!(default_policy, PrFeedbackPolicy::default());
    assert!(!default_policy.ingest);
    assert_eq!(default_policy.post_summary, PrFeedbackSummaryMode::Off);
}

#[tokio::test]
async fn v173_credential_reference_requires_a_configured_github_handle_of_the_owner() {
    let f = fixture().await;
    let other = seed_user(&f.db).await;
    let github = credential(&f.db, &f.user_id, "github", "configured").await;
    let model = credential(&f.db, &f.user_id, "openai", "configured").await;
    let revoked = credential(&f.db, &f.user_id, "github", "revoked").await;
    let reference = |handle: &str, owner: &str| {
        Some(PrProviderCredentialRef {
            handle_id: handle.to_owned(),
            owner_user_id: owner.to_owned(),
        })
    };
    for (handle, owner) in [
        (&model, &f.user_id),
        (&revoked, &f.user_id),
        (&github, &other),
    ] {
        assert!(
            PrProviderConfigRepo::set_credential(
                &f.db,
                &f.config_id,
                reference(handle, owner),
                &now_rfc3339()
            )
            .await
            .is_err(),
            "invalid credential reference is rejected"
        );
    }
    let config = PrProviderConfigRepo::set_credential(
        &f.db,
        &f.config_id,
        reference(&github, &f.user_id),
        "2030-01-01T00:00:00Z",
    )
    .await
    .expect("exact github handle is accepted");
    assert_eq!(
        config.credential_handle_id.as_deref(),
        Some(github.as_str())
    );
    assert_eq!(config.updated_at, "2030-01-01T00:00:00Z");
    // A referenced handle cannot be retargeted or deleted.
    assert!(
        sqlx::query("UPDATE credential_handle SET provider = 'openai' WHERE id = ?")
            .bind(&github)
            .execute(f.db.pool())
            .await
            .is_err()
    );
    assert!(sqlx::query("DELETE FROM credential_handle WHERE id = ?")
        .bind(&github)
        .execute(f.db.pool())
        .await
        .is_err());
    let cleared = PrProviderConfigRepo::set_credential(&f.db, &f.config_id, None, &now_rfc3339())
        .await
        .expect("clearing is allowed");
    assert!(cleared.credential_handle_id.is_none() && cleared.credential_owner_user_id.is_none());
}

#[tokio::test]
async fn v173_feedback_policy_is_validated_and_agent_profiles_reject_scm_credentials() {
    let f = fixture().await;
    let invalid_policies = vec![
        "{}".to_owned(),
        r#"{"schema_version":2}"#.to_owned(),
        policy(true, "force", "off"),
        policy(true, "automatic", "sometimes"),
    ];
    for bad in &invalid_policies {
        assert!(
            PrProviderConfigRepo::set_feedback_policy(&f.db, &f.config_id, bad, &now_rfc3339())
                .await
                .is_err(),
            "{bad} is rejected"
        );
    }
    let good = policy(true, "automatic", "human_approval");
    let config =
        PrProviderConfigRepo::set_feedback_policy(&f.db, &f.config_id, &good, &now_rfc3339())
            .await
            .expect("valid policy");
    assert_eq!(config.feedback_policy_json, good);

    let identity = seed_agent(&f.db, "scm-guard", "global", None).await;
    let github = credential(&f.db, &f.user_id, "github", "configured").await;
    let model = credential(&f.db, &f.user_id, "openai", "configured").await;
    let insert_profile = |credential_ref: Option<&str>| {
        let identity = identity.clone();
        let credential_ref = credential_ref.map(str::to_owned);
        let db = &f.db;
        async move {
            let now = now_rfc3339();
            sqlx::query(
                "INSERT INTO agent_profile (id, identity_id, backend_kind, executor_type,
                    credential_ref, created_at, updated_at)
                 VALUES (?, ?, 'native', 'embedded', ?, ?, ?)",
            )
            .bind(new_uuid_v4())
            .bind(identity)
            .bind(credential_ref)
            .bind(&now)
            .bind(&now)
            .execute(db.pool())
            .await
        }
    };
    assert!(insert_profile(Some(&github)).await.is_err());
    insert_profile(Some(&model))
        .await
        .expect("a model credential still backs a profile");
    insert_profile(None).await.expect("no credential");
    let profile_id = new_uuid_v4();
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO agent_profile (id, identity_id, backend_kind, executor_type, created_at, updated_at)
         VALUES (?, ?, 'native', 'embedded', ?, ?)",
    )
    .bind(&profile_id)
    .bind(&identity)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("profile");
    assert!(
        sqlx::query("UPDATE agent_profile SET credential_ref = ? WHERE id = ?")
            .bind(&github)
            .bind(&profile_id)
            .execute(f.db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn v173_external_feedback_is_idempotent_frozen_by_policy_and_immutable() {
    let f = fixture().await;
    seed_admission(
        &f,
        "open-ingest",
        "open",
        Some(&policy(true, "human_approval", "off")),
    )
    .await;
    seed_admission(
        &f,
        "open-no-ingest",
        "open",
        Some(&policy(false, "human_approval", "off")),
    )
    .await;
    seed_admission(&f, "legacy", "open", None).await;
    seed_admission(
        &f,
        "closed",
        "closed",
        Some(&policy(true, "human_approval", "off")),
    )
    .await;

    for admission in ["open-no-ingest", "legacy", "closed"] {
        let error = ExternalFeedbackRepo::ingest_external_feedback(
            &f.db,
            vec![feedback_input(&f, admission, "c1", "body")],
        )
        .await
        .expect_err("ingestion needs the open admission with ingest in its frozen policy");
        assert!(matches!(error, DbError::Check(_)), "{admission}: {error:?}");
    }

    let first = feedback_input(&f, "open-ingest", "c1", "please fix");
    let ingested = ExternalFeedbackRepo::ingest_external_feedback(&f.db, vec![first.clone()])
        .await
        .expect("ingest");
    assert!(ingested[0].1);
    let replay = ExternalFeedbackRepo::ingest_external_feedback(
        &f.db,
        vec![feedback_input(&f, "open-ingest", "c1", "please fix")],
    )
    .await
    .expect("replay");
    assert!(!replay[0].1);
    assert_eq!(
        replay[0].0.id, first.id,
        "the replay returns the original row"
    );
    let edited = ExternalFeedbackRepo::ingest_external_feedback(
        &f.db,
        vec![feedback_input(&f, "open-ingest", "c1", "edited body")],
    )
    .await
    .expect("an edit is a new immutable row");
    assert!(edited[0].1);
    assert_eq!(
        ExternalFeedbackRepo::list_external_feedback_items(
            &f.db,
            "open-ingest",
            None,
            None,
            &page(10)
        )
        .await
        .expect("list")
        .items
        .len(),
        2
    );

    assert!(
        sqlx::query("UPDATE external_feedback_item SET body = 'changed' WHERE id = ?")
            .bind(&first.id)
            .execute(f.db.pool())
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM external_feedback_item WHERE id = ?")
            .bind(&first.id)
            .execute(f.db.pool())
            .await
            .is_err()
    );

    // A different head makes earlier comments stale (one way only).
    let marked = ExternalFeedbackRepo::mark_external_feedback_stale_head(
        &f.db,
        "open-ingest",
        &"b".repeat(40),
    )
    .await
    .expect("mark stale");
    assert_eq!(marked, 2);
    assert!(
        ExternalFeedbackRepo::list_unconsumed_trusted_feedback(&f.db, "open-ingest", 20)
            .await
            .expect("candidates")
            .is_empty()
    );
    assert!(
        sqlx::query("UPDATE external_feedback_item SET head_state = 'current' WHERE id = ?")
            .bind(&first.id)
            .execute(f.db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn v173_external_feedback_requests_triage_each_item_of_the_exact_set() {
    let f = fixture().await;
    let now = now_rfc3339();
    let policy_json = policy(true, "human_approval", "off");
    seed_admission(&f, "adm", "open", Some(&policy_json)).await;
    // Authorized coordination for the Task (as the phase 1 fixtures do).
    let role_id = new_uuid_v4();
    TaskRoleRepo::create(
        &f.db,
        CreateTaskRole {
            id: role_id.clone(),
            task_id: f.task_id.clone(),
            role: "coordinator".to_owned(),
            coordination_mode: None,
            policy_json: "{}".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("coordinator role");
    sqlx::query(
        "UPDATE task_coordination SET mode = 'prepare_and_execute', coordinator_task_role_id = ?,
            coordinator_actor_kind = 'agent', coordinator_actor_id = 'agent-v173',
            authorization_version = 1, envelope_json = '{\"schema_version\":2}',
            envelope_digest = ?, team_digest = ?, requested_by = ? WHERE task_id = ?",
    )
    .bind(&role_id)
    .bind(digest64("envelope"))
    .bind(digest64("team"))
    .bind(&f.user_id)
    .bind(&f.task_id)
    .execute(f.db.pool())
    .await
    .expect("authorize");
    sqlx::query("UPDATE task_coordination SET team_stale = 0 WHERE task_id = ?")
        .bind(&f.task_id)
        .execute(f.db.pool())
        .await
        .expect("settle team");
    let execution_id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO execution (id, task_id, status, created_at, updated_at)
         VALUES (?, ?, 'completed', ?, ?)",
    )
    .bind(&execution_id)
    .bind(&f.task_id)
    .bind(&now)
    .bind(&now)
    .execute(f.db.pool())
    .await
    .expect("execution");

    let items = ExternalFeedbackRepo::ingest_external_feedback(
        &f.db,
        vec![
            feedback_input(&f, "adm", "c1", "one"),
            feedback_input(&f, "adm", "c2", "two"),
        ],
    )
    .await
    .expect("ingest")
    .into_iter()
    .map(|(item, _)| item)
    .collect::<Vec<_>>();
    let entries = items
        .iter()
        .map(|item| ExternalFeedbackSetEntry {
            feedback_item_id: item.id.clone(),
            body_digest: item.body_digest.clone(),
        })
        .collect::<Vec<_>>();

    let insert_request = |id: &str, source_id: &str| {
        let id = id.to_owned();
        let source_id = source_id.to_owned();
        let pool = f.db.pool().clone();
        let task_id = f.task_id.clone();
        async move {
            let now = now_rfc3339();
            sqlx::query(
                "INSERT INTO task_coordination_request (
                    id, task_id, authorization_version, mode, brief_json, objective_digest,
                    request_digest, idempotency_key, state, version, created_at, updated_at,
                    kind, source_subject_kind, source_subject_id, source_subject_digest
                 ) VALUES (?, ?, 1, 'prepare_and_execute', '{}', ?, ?, ?, 'requested', 1, ?, ?,
                           'external_feedback', 'external_feedback_set', ?, ?)",
            )
            .bind(&id)
            .bind(task_id)
            .bind(digest64("objective"))
            .bind(digest64("request"))
            .bind(format!("key-{id}"))
            .bind(&now)
            .bind(&now)
            .bind(source_id)
            .bind(digest64("set"))
            .execute(&pool)
            .await
        }
    };
    assert!(
        insert_request(&new_uuid_v4(), "no-such-admission")
            .await
            .is_err(),
        "the source must be this Task's open admission"
    );
    let request_id = new_uuid_v4();
    insert_request(&request_id, "adm").await.expect("request");
    ExternalFeedbackRepo::add_external_feedback_set_items(&f.db, &request_id, &f.task_id, &entries)
        .await
        .expect("set");
    ExternalFeedbackRepo::add_external_feedback_set_items(&f.db, &request_id, &f.task_id, &entries)
        .await
        .expect("identical replay");
    assert!(
        ExternalFeedbackRepo::add_external_feedback_set_items(
            &f.db,
            &request_id,
            &f.task_id,
            &entries[..1]
        )
        .await
        .is_err(),
        "a different set is rejected"
    );
    let set = ExternalFeedbackRepo::list_external_feedback_set(&f.db, &request_id)
        .await
        .expect("set");
    assert_eq!(
        set.iter().map(|i| &i.id).collect::<Vec<_>>(),
        items.iter().map(|i| &i.id).collect::<Vec<_>>()
    );
    assert!(
        ExternalFeedbackRepo::list_unconsumed_trusted_feedback(&f.db, "adm", 20)
            .await
            .expect("candidates")
            .is_empty(),
        "consumed items are not offered again"
    );

    sqlx::query(
        "UPDATE task_coordination_request SET state = 'running', version = 2, execution_id = ?
         WHERE id = ?",
    )
    .bind(&execution_id)
    .bind(&request_id)
    .execute(f.db.pool())
    .await
    .expect("running");
    let triaged = |request_id: &str| {
        sqlx::query(
            "UPDATE task_coordination_request SET state = 'triaged', version = 3 WHERE id = ?",
        )
        .bind(request_id.to_owned())
        .execute(f.db.pool())
    };
    assert!(
        triaged(&request_id).await.is_err(),
        "triage needs every item"
    );
    let decision = |item: &crate::ExternalFeedbackItem, disposition| CreateExternalFeedbackTriage {
        id: new_uuid_v4(),
        task_id: f.task_id.clone(),
        request_id: request_id.clone(),
        feedback_item_id: item.id.clone(),
        body_digest: item.body_digest.clone(),
        disposition,
        rationale: "because".to_owned(),
        instructions: "do it".to_owned(),
        decision_digest: digest64(&item.id),
        execution_id: execution_id.clone(),
        created_at: now_rfc3339(),
    };
    let first = ExternalFeedbackRepo::record_external_feedback_triage(
        &f.db,
        decision(&items[0], ExternalFeedbackDisposition::Correct),
    )
    .await
    .expect("first decision");
    let replay = ExternalFeedbackRepo::record_external_feedback_triage(
        &f.db,
        decision(&items[0], ExternalFeedbackDisposition::Correct),
    )
    .await
    .expect("identical replay returns the decision");
    assert_eq!(replay.id, first.id);
    assert!(matches!(
        ExternalFeedbackRepo::record_external_feedback_triage(
            &f.db,
            decision(&items[0], ExternalFeedbackDisposition::Dismiss)
        )
        .await,
        Err(DbError::IdempotencyConflict)
    ));
    assert!(
        triaged(&request_id).await.is_err(),
        "one decision is missing"
    );
    ExternalFeedbackRepo::record_external_feedback_triage(
        &f.db,
        decision(&items[1], ExternalFeedbackDisposition::Dismiss),
    )
    .await
    .expect("second decision");
    triaged(&request_id)
        .await
        .expect("every item has its decision");
    assert_eq!(
        ExternalFeedbackRepo::list_external_feedback_triage(&f.db, &request_id)
            .await
            .expect("triage")
            .len(),
        2
    );
    assert!(
        sqlx::query("UPDATE task_external_feedback_triage SET rationale = 'x'")
            .execute(f.db.pool())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn v173_pr_external_actions_follow_the_frozen_policy_and_forward_only_states() {
    let f = fixture().await;
    seed_admission(
        &f,
        "manual",
        "open",
        Some(&policy(true, "human_approval", "off")),
    )
    .await;
    seed_admission(
        &f,
        "auto",
        "open",
        Some(&policy(true, "human_approval", "automatic")),
    )
    .await;
    // Artifact/producer guards are covered by their own migrations.
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = 'artifact'
           AND sql LIKE '%BEFORE INSERT%'",
    )
    .fetch_all(f.db.pool())
    .await
    .expect("triggers");
    for name in names {
        sqlx::query(&format!("DROP TRIGGER {name}"))
            .execute(f.db.pool())
            .await
            .expect("drop");
    }
    sqlx::query(
        "INSERT INTO artifact (id, task_id, kind, storage_kind, content, created_at)
         VALUES ('summary', ?, 'summary', 'inline', 'text', ?)",
    )
    .bind(&f.task_id)
    .bind(now_rfc3339())
    .execute(f.db.pool())
    .await
    .expect("artifact");
    let comment =
        |admission: &str, mode: PrFeedbackAuthorization, key: &str| CreatePrExternalAction {
            id: new_uuid_v4(),
            task_id: f.task_id.clone(),
            admission_id: admission.to_owned(),
            kind: PrExternalActionKind::PostComment,
            authorization_mode: mode,
            gate_evaluation_id: None,
            subject_commit_sha: None,
            expected_remote_head_sha: None,
            summary_artifact_id: Some("summary".to_owned()),
            body_digest: Some(digest64("body")),
            idempotency_key: key.to_owned(),
            created_at: now_rfc3339(),
        };
    // post_summary = off forbids even a human-approved comment; automatic mode
    // needs the frozen policy to say automatic for that kind.
    for (admission, mode) in [
        ("manual", PrFeedbackAuthorization::HumanApproval),
        ("manual", PrFeedbackAuthorization::Automatic),
    ] {
        assert!(matches!(
            PrExternalActionRepo::create_pr_external_action(
                &f.db,
                comment(admission, mode, "k0"),
                None
            )
            .await,
            Err(DbError::PrActionNotAuthorized)
        ));
    }
    let created = PrExternalActionRepo::create_pr_external_action(
        &f.db,
        comment("auto", PrFeedbackAuthorization::Automatic, "k1"),
        None,
    )
    .await
    .expect("automatic comment under the frozen automatic policy");
    assert!(created.created);
    assert_eq!(created.action.state, PrExternalActionState::Approved);
    let replay = PrExternalActionRepo::create_pr_external_action(
        &f.db,
        comment("auto", PrFeedbackAuthorization::Automatic, "k1"),
        None,
    )
    .await
    .expect("identical idempotent replay");
    assert!(!replay.created);
    assert_eq!(replay.action.id, created.action.id);
    let action = created.action;
    // One open action per Task.
    assert!(PrExternalActionRepo::create_pr_external_action(
        &f.db,
        comment("auto", PrFeedbackAuthorization::Automatic, "k2"),
        None
    )
    .await
    .is_err());
    let step = |action: &PrExternalAction, to_state| TransitionPrExternalAction {
        id: action.id.clone(),
        expected_version: action.version,
        to_state,
        decision_id: None,
        provider_event_id: None,
        provider_result_json: None,
        updated_at: "2030-01-01T00:00:00Z".to_owned(),
    };
    let running = PrExternalActionRepo::transition_pr_external_action(
        &f.db,
        step(&action, PrExternalActionState::Running),
        None,
    )
    .await
    .expect("start")
    .action;
    // A stale version never applies; an illegal edge never applies.
    assert!(matches!(
        PrExternalActionRepo::transition_pr_external_action(
            &f.db,
            step(&action, PrExternalActionState::Failed),
            None
        )
        .await,
        Err(DbError::VersionConflict)
    ));
    assert!(PrExternalActionRepo::transition_pr_external_action(
        &f.db,
        step(&running, PrExternalActionState::Approved),
        None
    )
    .await
    .is_err());
    let unknown = PrExternalActionRepo::transition_pr_external_action(
        &f.db,
        step(&running, PrExternalActionState::OutcomeUnknown),
        None,
    )
    .await
    .expect("uncertain provider result")
    .action;
    assert!(unknown.finished_at.is_none());
    assert!(
        PrExternalActionRepo::transition_pr_external_action(
            &f.db,
            step(&unknown, PrExternalActionState::Running),
            None
        )
        .await
        .is_err(),
        "no blind retry after an uncertain outcome"
    );
    let done = PrExternalActionRepo::transition_pr_external_action(
        &f.db,
        step(&unknown, PrExternalActionState::Succeeded),
        None,
    )
    .await
    .expect("reconciled")
    .action;
    assert!(done.finished_at.is_some());
    assert!(PrExternalActionRepo::transition_pr_external_action(
        &f.db,
        step(&done, PrExternalActionState::Failed),
        None
    )
    .await
    .is_err());
    assert!(
        PrExternalActionRepo::get_open_pr_external_action(&f.db, &f.task_id)
            .await
            .expect("open")
            .is_none()
    );
    assert!(sqlx::query("DELETE FROM pr_external_action WHERE id = ?")
        .bind(&done.id)
        .execute(f.db.pool())
        .await
        .is_err());
}
