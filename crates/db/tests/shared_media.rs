use db::{
    create_sqlite_pool, now_rfc3339, run_migrations, BeginProjectMediaUpload, CommentAuthorType,
    CreateProjectMediaAsset, CreateTaskMedia, ProjectMediaTombstone, SharedMediaRepo, SqliteDb,
    TaskMediaRepo,
};
use sqlx::SqlitePool;

async fn fixture() -> (SqliteDb, String, String) {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    fixture_rows(pool).await
}

async fn fixture_rows(pool: SqlitePool) -> (SqliteDb, String, String) {
    let db = SqliteDb::new(pool);
    let project_id = "project-media".to_owned();
    let task_id = "task-media".to_owned();
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, display_name, created_at, updated_at)
         VALUES ('user-media', 'media@example.test', 'test', 'Media Tester', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("user");
    sqlx::query(
        "INSERT INTO project (id, name, owner_id, settings, created_at, updated_at)
         VALUES (?, ?, 'user-media', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind("Media Project")
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("project");
    sqlx::query(
        "INSERT INTO repo (id, project_id, name, remote_url, local_path, work_mode, default_branch, created_at, updated_at)
         VALUES ('repo-media', ?, 'Media Repo', '/tmp/media-repo', '/tmp/media-repo', 'direct_merge', 'main', ?, ?)",
    )
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("repo");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, task_type, status, created_at, updated_at)
         VALUES (?, ?, 'repo-media', 'Media Task', 'implementation', 'todo', ?, ?)",
    )
    .bind(&task_id)
    .bind(&project_id)
    .bind(&now)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("task");
    (db, project_id, task_id)
}

async fn upload(db: &SqliteDb, task_id: &str, id: &str) -> db::TaskMedia {
    let media = TaskMediaRepo::create_media(
        db,
        CreateTaskMedia {
            id: id.to_owned(),
            task_id: task_id.to_owned(),
            display_filename: "proof.png".to_owned(),
            content_type: "image/png".to_owned(),
            byte_size: 4,
            storage_key: format!("{task_id}/{id}__proof.png"),
            author_type: CommentAuthorType::User,
            author_id: Some("user-media".to_owned()),
            author_name: "Tester".to_owned(),
            created_at: now_rfc3339(),
        },
    )
    .await
    .expect("media");
    // The focused repository tests model a legacy row whose unchanged bytes
    // have already gone through checksum reconciliation. Production callers
    // perform that same CAS only after hashing the stored bytes.
    SharedMediaRepo::set_media_asset_checksum(
        db,
        &media.id,
        media.byte_size,
        "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
        &now_rfc3339(),
    )
    .await
    .expect("legacy media checksum reconciliation");
    media
}

#[tokio::test]
async fn unreferenced_task_media_is_claimed_and_gc_is_idempotent() {
    let (db, _project_id, task_id) = fixture().await;
    let media = upload(&db, &task_id, "asset-gc").await;
    let deleted_at = now_rfc3339();
    TaskMediaRepo::soft_delete_media(&db, &media.id, &deleted_at)
        .await
        .expect("soft delete");

    let asset = SharedMediaRepo::get_media_asset(&db, &media.id)
        .await
        .expect("asset")
        .expect("asset row");
    assert_eq!(asset.gc_state, "gc_candidate");
    let candidates = SharedMediaRepo::claim_media_gc_candidates(
        &db,
        &deleted_at,
        "worker-gc-1",
        "9999-12-31T00:00:00Z",
        10,
    )
    .await
    .expect("claim");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].gc_state, "gc_queued");
    assert_eq!(candidates[0].gc_lease_owner.as_deref(), Some("worker-gc-1"));
    assert_eq!(
        candidates[0].gc_lease_expires_at.as_deref(),
        Some("9999-12-31T00:00:00Z")
    );
    let claimed_version = candidates[0].version;

    let completed = SharedMediaRepo::complete_media_gc(
        &db,
        &media.id,
        "worker-gc-1",
        claimed_version,
        &deleted_at,
    )
    .await
    .expect("complete")
    .expect("completed asset");
    assert_eq!(completed.gc_state, "deleted");
    assert_eq!(completed.availability, "purged");
    assert!(SharedMediaRepo::claim_media_gc_candidates(
        &db,
        &deleted_at,
        "worker-gc-1",
        "9999-12-31T00:00:00Z",
        10,
    )
    .await
    .expect("replay claim")
    .is_empty());
    assert!(matches!(
        SharedMediaRepo::complete_media_gc(
            &db,
            &media.id,
            "worker-gc-1",
            claimed_version,
            &deleted_at,
        )
        .await,
        Err(db::DbError::VersionConflict)
    ));
}

#[tokio::test]
async fn immutable_release_pin_keeps_asset_referenced_after_task_cleanup() {
    let (db, project_id, task_id) = fixture().await;
    let media = upload(&db, &task_id, "asset-pinned").await;
    let now = now_rfc3339();
    // ProjectRelease is immutable history with no writer. Seed a representative
    // historic row and its copied pin below the released write fences so this
    // test can exercise media retention against the V122 schema.
    sqlx::query("DROP TRIGGER pr11_retired_insert_project_release")
        .execute(db.pool())
        .await
        .expect("temporarily open fixture insert");
    sqlx::query(
        "INSERT INTO project_release (
            id, project_id, milestone_id, release_sequence, release_revision,
            release_identifier, milestone_revision_id, readiness_snapshot_id,
            readiness_digest, baseline_id, baseline_revision_id, baseline_digest,
            release_policy_revision, release_policy_digest, releasing_principal_type,
            releasing_principal_id, authorization_basis, authorization_action,
            explicit_event, authorization_occurred_at, schema_version,
            snapshot_digest, idempotency_key, created_at
         ) VALUES (
            'release-media', ?, 'milestone-identity', 1, 1, 'M001-r1',
            'milestone-revision-identity', 'readiness-identity', 'readiness-digest',
            'baseline-identity', 'baseline-revision-identity', 'baseline-digest',
            'policy-revision', 'policy-digest', 'user', 'user-media', 'test',
            'project.release.create', 'release-event', '2026-08-13T00:00:00Z',
            'release-v1', 'snapshot-digest', 'release-media-key', ?
         )",
    )
    .bind(&project_id)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("historic Release fixture");
    sqlx::query(
        "CREATE TRIGGER pr11_retired_insert_project_release
         BEFORE INSERT ON project_release BEGIN
             SELECT RAISE(ABORT, 'PR11_OPERATION_RETIRED: Project Release');
         END",
    )
    .execute(db.pool())
    .await
    .expect("restore Release write fence");
    sqlx::query("DROP TRIGGER project_release_history_media_pin_insert_guard")
        .execute(db.pool())
        .await
        .expect("temporarily open history fixture insert");
    sqlx::query(
        "INSERT INTO project_release_history_media_pin (
            id, project_id, release_id, asset_id, attachment_id,
            legacy_task_media_id, asset_checksum, attachment_digest,
            availability, pin_digest, created_at
         ) VALUES (
            'pin-media', ?, 'release-media', ?, NULL, ?,
            '9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08',
            'attachment-digest', 'available', 'pin-digest', ?
         )",
    )
    .bind(&project_id)
    .bind(&media.id)
    .bind(&media.id)
    .bind(&now)
    .execute(db.pool())
    .await
    .expect("historic Release pin fixture");
    sqlx::query(
        "CREATE TRIGGER project_release_history_media_pin_insert_guard
         BEFORE INSERT ON project_release_history_media_pin BEGIN
             SELECT RAISE(ABORT, 'ProjectRelease history snapshots are immutable');
         END",
    )
    .execute(db.pool())
    .await
    .expect("restore history write fence");
    let retained_pin: (String, String, String, String) = sqlx::query_as(
        "SELECT id, asset_id, asset_checksum, pin_digest
         FROM project_release_history_media_pin WHERE release_id = 'release-media'",
    )
    .fetch_one(db.pool())
    .await
    .expect("copied historical media pin");
    assert_eq!(
        retained_pin,
        (
            "pin-media".to_owned(),
            media.id.clone(),
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08".to_owned(),
            "pin-digest".to_owned(),
        )
    );
    let source_pin_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master
         WHERE type = 'table' AND name = 'project_release_media_pin'",
    )
    .fetch_one(db.pool())
    .await
    .expect("source pin table lookup");
    assert_eq!(source_pin_table, 0);

    TaskMediaRepo::soft_delete_media(&db, &media.id, &now)
        .await
        .expect("soft delete task media");
    assert!(TaskMediaRepo::get_media_by_id(&db, &media.id, false)
        .await
        .expect("task media lookup")
        .is_none());
    let asset = SharedMediaRepo::get_media_asset(&db, &media.id)
        .await
        .expect("asset")
        .expect("asset row");
    assert_eq!(asset.gc_state, "referenced");
    assert_eq!(
        SharedMediaRepo::list_project_release_media_pins(&db, "release-media")
            .await
            .expect("pins")
            .len(),
        1
    );
    assert!(SharedMediaRepo::claim_media_gc_candidates(
        &db,
        &now,
        "worker-pinned",
        "9999-12-31T00:00:00Z",
        10,
    )
    .await
    .expect("claim pinned")
    .is_empty());
}

#[tokio::test]
async fn project_upload_staging_is_replay_safe_and_version_bound() {
    let (db, project_id, _task_id) = fixture().await;
    let now = now_rfc3339();
    let pending = SharedMediaRepo::begin_project_media_upload(
        &db,
        BeginProjectMediaUpload {
            project_id: project_id.clone(),
            idempotency_key: "upload-replay".to_owned(),
            mutation_fingerprint: "fingerprint-1".to_owned(),
            expected_project_version: 1,
            asset_id: "asset-project-upload".to_owned(),
            final_storage_key: "projects/project-media/asset-project-upload__proof.png".to_owned(),
            staging_storage_key:
                "pending/projects/project-media/asset-project-upload__proof.png.uploading"
                    .to_owned(),
            display_filename: "proof.png".to_owned(),
            content_type: "image/png".to_owned(),
            byte_size: 8,
            checksum: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("pending upload");
    assert_eq!(pending.status, "pending");

    let quarantined = SharedMediaRepo::create_project_media_asset(
        &db,
        CreateProjectMediaAsset {
            id: pending.asset_id.clone(),
            project_id: project_id.clone(),
            display_filename: pending.display_filename.clone(),
            content_type: pending.content_type.clone(),
            byte_size: pending.byte_size,
            storage_key: pending.final_storage_key.clone(),
            checksum: pending.checksum.clone(),
            idempotency_key: pending.idempotency_key.clone(),
            mutation_fingerprint: pending.mutation_fingerprint.clone(),
            expected_project_version: pending.expected_project_version,
            actor_type: "user".to_owned(),
            actor_id: Some("user-media".to_owned()),
            authorization_event_id: "auth-upload".to_owned(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("metadata commit");
    assert_eq!(quarantined.availability, "quarantined");
    let available =
        SharedMediaRepo::finalize_project_media_upload(&db, &project_id, &pending.asset_id, &now)
            .await
            .expect("finalize");
    assert_eq!(available.availability, "available");

    let replay = SharedMediaRepo::begin_project_media_upload(
        &db,
        BeginProjectMediaUpload {
            project_id: project_id.clone(),
            idempotency_key: pending.idempotency_key.clone(),
            mutation_fingerprint: pending.mutation_fingerprint.clone(),
            expected_project_version: pending.expected_project_version,
            asset_id: "a-new-server-id-is-ignored".to_owned(),
            final_storage_key: "a-new-storage-key-is-ignored".to_owned(),
            staging_storage_key: "a-new-staging-key-is-ignored".to_owned(),
            display_filename: pending.display_filename.clone(),
            content_type: pending.content_type.clone(),
            byte_size: pending.byte_size,
            checksum: pending.checksum.clone(),
            created_at: now.clone(),
        },
    )
    .await
    .expect("exact replay");
    assert_eq!(replay.status, "finalized");
    assert_eq!(replay.asset_id, pending.asset_id);

    let conflict = SharedMediaRepo::begin_project_media_upload(
        &db,
        BeginProjectMediaUpload {
            project_id,
            idempotency_key: pending.idempotency_key,
            mutation_fingerprint: "different-fingerprint".to_owned(),
            expected_project_version: 1,
            asset_id: "asset-conflict".to_owned(),
            final_storage_key: "conflict".to_owned(),
            staging_storage_key: "conflict-staging".to_owned(),
            display_filename: pending.display_filename,
            content_type: pending.content_type,
            byte_size: pending.byte_size,
            checksum: pending.checksum,
            created_at: now,
        },
    )
    .await;
    assert!(matches!(conflict, Err(db::DbError::IdempotencyConflict)));
}

#[tokio::test]
async fn project_media_tombstone_replay_compares_complete_user_receipt() {
    let (db, project_id, task_id) = fixture().await;
    let media = upload(&db, &task_id, "asset-tombstone-replay").await;
    let media_asset = SharedMediaRepo::get_media_asset(&db, &media.id)
        .await
        .expect("shared media asset")
        .expect("shared media asset row");
    let created_at = now_rfc3339();
    let original = ProjectMediaTombstone {
        asset_id: media.id.clone(),
        project_id: project_id.clone(),
        expected_version: media_asset.version,
        idempotency_key: "media-tombstone-replay".to_owned(),
        mutation_fingerprint: "media-tombstone-request".to_owned(),
        target_availability: "redacted".to_owned(),
        principal_type: "user".to_owned(),
        principal_id: "user-media".to_owned(),
        authorization_basis: "explicit_user_authorization".to_owned(),
        authorization_action: "project.media.redact".to_owned(),
        authorization_occurred_at: "2026-08-13T00:00:00Z".to_owned(),
        authorization_event_id: "media-tombstone-event".to_owned(),
        authorization_json: serde_json::json!({
            "principal": {"kind": "user", "id": "user-media"},
            "authorization_basis": "explicit_user_authorization",
            "action": "project.media.redact",
            "event_id": "media-tombstone-event",
            "occurred_at": "2026-08-13T00:00:00Z"
        })
        .to_string(),
        reason: "remove the bounded proof from active media".to_owned(),
        created_at,
    };
    let first = SharedMediaRepo::tombstone_project_media_asset(&db, original.clone())
        .await
        .expect("media tombstone");
    assert_eq!(first.id, media.id);
    assert_eq!(first.availability, "redacted");
    let replay = SharedMediaRepo::tombstone_project_media_asset(&db, original.clone())
        .await
        .expect("exact media tombstone replay");
    assert_eq!(replay, first);
    let replay_projection = SharedMediaRepo::replay_project_media_tombstone(&db, original.clone())
        .await
        .expect("read-only media tombstone replay")
        .expect("committed media tombstone receipt");
    assert_eq!(replay_projection, first);

    // The route resolves a receipt before current authorization validation;
    // even a malformed changed authority must therefore conflict rather than
    // become a new 400/403 mutation attempt.
    let mut malformed_authority = original.clone();
    malformed_authority.authorization_occurred_at = "not-rfc3339".to_owned();
    let conflict = SharedMediaRepo::replay_project_media_tombstone(&db, malformed_authority).await;
    assert!(matches!(conflict, Err(db::DbError::IdempotencyConflict)));

    // The repository receives the canonical mutation fingerprint from the
    // API, but it must still compare each persisted authority field directly.
    // Keeping the fingerprint/key constant here isolates the receipt fields.
    for (label, altered) in [
        ("tombstone action", {
            let mut value = original.clone();
            value.authorization_action = "project.media.purge".to_owned();
            value
        }),
        ("tombstone occurred_at", {
            let mut value = original.clone();
            value.authorization_occurred_at = "2026-08-13T00:00:01Z".to_owned();
            value
        }),
        ("tombstone basis", {
            let mut value = original.clone();
            value.authorization_basis = "altered_basis".to_owned();
            value
        }),
        ("tombstone event", {
            let mut value = original.clone();
            value.authorization_event_id = "altered-tombstone-event".to_owned();
            value
        }),
        ("tombstone principal", {
            let mut value = original.clone();
            value.principal_id = "different-user".to_owned();
            value
        }),
        ("tombstone target asset", {
            let mut value = original.clone();
            value.asset_id = "different-media-asset".to_owned();
            value
        }),
        ("tombstone target version", {
            let mut value = original.clone();
            value.expected_version += 1;
            value
        }),
        ("tombstone target disposition", {
            let mut value = original.clone();
            value.target_availability = "purged".to_owned();
            value
        }),
    ] {
        let conflict = SharedMediaRepo::tombstone_project_media_asset(&db, altered).await;
        assert!(
            matches!(conflict, Err(db::DbError::IdempotencyConflict)),
            "{label} must conflict on replay, got {conflict:?}"
        );
    }
    let tombstone_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM media_asset_tombstone WHERE idempotency_key = ?")
            .bind(&original.idempotency_key)
            .fetch_one(db.pool())
            .await
            .expect("one immutable tombstone row");
    assert_eq!(tombstone_count, 1);
}

#[tokio::test]
async fn queued_gc_is_restartable_and_rejects_forged_release_pins() {
    let (db, project_id, task_id) = fixture().await;
    let media = upload(&db, &task_id, "asset-restart").await;
    let now = now_rfc3339();
    TaskMediaRepo::soft_delete_media(&db, &media.id, &now)
        .await
        .expect("soft delete");
    let first_claim = SharedMediaRepo::claim_media_gc_candidates(
        &db,
        &now,
        "worker-restart-1",
        "9999-12-31T00:00:00Z",
        1,
    )
    .await
    .expect("claim");
    assert_eq!(first_claim.len(), 1);
    let first_claim_version = first_claim[0].version;

    // Release history has no public writer after the V121/V122 cutover. Its
    // schema fence must still reject a forged pin while GC owns the asset.
    let historical_pin_write = sqlx::query(
        "INSERT INTO project_release_history_media_pin
            (id, project_id, release_id, asset_id, attachment_id,
             legacy_task_media_id, asset_checksum, attachment_digest,
             availability, pin_digest, created_at)
         VALUES ('forged-history', ?, 'missing-release', ?, NULL, NULL,
                 'checksum', 'attachment', 'available', 'digest', ?)",
    )
    .bind(&project_id)
    .bind(&media.id)
    .bind(&now)
    .execute(db.pool())
    .await;
    assert!(
        historical_pin_write.is_err(),
        "schema guard must reject forged release history pins"
    );

    // A worker restart sees the queued row again, and reconciliation remains
    // idempotent even when the first worker never finalized it.
    // Simulate an expired persisted lease without replaying state.  A new
    // worker can reclaim it only because the lease expiry is now in the past.
    sqlx::query(
        "UPDATE media_asset
         SET gc_lease_expires_at = '2000-01-01T00:00:00Z'
         WHERE id = ?",
    )
    .bind(&media.id)
    .execute(db.pool())
    .await
    .expect("expire worker lease");
    let restarted = SqliteDb::new(db.pool().clone());
    let candidates = SharedMediaRepo::claim_media_gc_candidates(
        &restarted,
        &now,
        "worker-restart-2",
        "9999-12-31T00:00:00Z",
        1,
    )
    .await
    .expect("restarted claim");
    assert_eq!(candidates.len(), 1);
    assert_eq!(candidates[0].id, media.id);
    assert!(matches!(
        SharedMediaRepo::complete_media_gc(
            &restarted,
            &media.id,
            "worker-restart-1",
            first_claim_version,
            &now,
        )
        .await,
        Err(db::DbError::VersionConflict)
    ));
    SharedMediaRepo::complete_media_gc(
        &restarted,
        &media.id,
        "worker-restart-2",
        candidates[0].version,
        &now,
    )
    .await
    .expect("restarted finalize")
    .expect("restarted tombstone");

    let unreconciled = SharedMediaRepo::list_purged_media_assets(&restarted, 32)
        .await
        .expect("purged reconciliation list");
    assert_eq!(unreconciled.len(), 1);
    assert_eq!(unreconciled[0].id, media.id);
    SharedMediaRepo::mark_purged_media_asset_reconciled(&restarted, &media.id, &now)
        .await
        .expect("purge reconciliation marker");
    assert!(SharedMediaRepo::list_purged_media_assets(&restarted, 32)
        .await
        .expect("advanced purge reconciliation list")
        .is_empty());
}

// Keep the import in this integration test explicit: it makes accidental use
// of a non-migrated connection obvious if the fixture is ever simplified.
#[allow(dead_code)]
fn _pool_type(_: &SqlitePool) {}
