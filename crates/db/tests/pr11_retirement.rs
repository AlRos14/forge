use db::{create_sqlite_pool, now_rfc3339, run_migrations, run_migrations_from};
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

#[tokio::test]
async fn v118_removes_database_bootstrap_and_fences_vertical_writes() {
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("in-memory database");
    run_migrations(&pool)
        .await
        .expect("all migrations including V118");

    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('pr11-user', 'pr11@example.test', 'fixture', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("new user");
    sqlx::query(
        "INSERT INTO project (id, name, settings, created_at, updated_at)
         VALUES ('pr11-project', 'PR11 empty Project', '{}', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("new empty Project");

    for table in [
        "agent_chat",
        "account_main_agent_binding",
        "project_agent_binding",
        "product_genesis_session",
        "project_charter",
        "task",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .expect("count after user and Project creation");
        assert_eq!(count, 0, "V118 must not create {table} implicitly");
    }

    let now = now_rfc3339();
    let denied = sqlx::query(
        "INSERT INTO agent_chat (id, kind, account_id, project_id, status, created_at, updated_at)
         VALUES ('pr11-forbidden-chat', 'account_main', 'pr11-user', NULL, 'active', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(denied.is_err(), "direct SQL cannot recreate Agent Chat");

    let ledger_rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pr11_vertical_migration_issue
         WHERE source_kind = 'role_membership'",
    )
    .fetch_one(&pool)
    .await
    .expect("migration issue ledger");
    assert_eq!(
        ledger_rows, 0,
        "empty Project creation invents no Task rows"
    );
}

#[tokio::test]
async fn pre_v071_room_history_is_converted_once_then_kept_historical_by_v118() {
    let temp = tempfile::tempdir().expect("temp dir");
    let migrations = temp.path().join("migrations");
    fs::create_dir_all(&migrations).expect("migration dir");
    copy_migrations_through(70, &migrations);
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("in-memory database");
    run_migrations_from(&pool, &migrations)
        .await
        .expect("pre-V071 schema");

    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('room-history-owner', 'room-history@example.test', 'fixture', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("pre-V071 user");
    sqlx::query(
        "INSERT INTO project (id, name, settings, owner_id, created_at, updated_at)
         VALUES ('room-history-project', 'Room history', '{}',
                 'room-history-owner', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("pre-V071 Project");
    sqlx::query(
        "INSERT INTO room
         (id, scope_type, scope_id, owner_user_id, title, created_at, updated_at)
         VALUES ('room-history-account', 'account', 'room-history-owner',
                 'room-history-owner', 'Old Room', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("pre-V071 Room");
    sqlx::query(
        "INSERT INTO room_message
         (id, room_id, author_type, author_id, content, status,
          correlation_id, sequence, created_at)
         VALUES ('room-history-message', 'room-history-account', 'user',
                 'room-history-owner', 'Room transcript remains historical',
                 'complete', 'room-history-correlation', 1, ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("pre-V071 Room message");

    copy_migrations_through(118, &migrations);
    run_migrations_from(&pool, &migrations)
        .await
        .expect("Room conversion and PR11 retirement");
    run_migrations_from(&pool, &migrations)
        .await
        .expect("restart does not reconvert Room history");

    let migrated: (i64, String) = sqlx::query_as(
        "SELECT COUNT(*), MIN(content) FROM agent_chat_message
         WHERE id = 'room-history-message' AND source_type = 'room'
           AND source_id = 'room-history-account'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical Room transcript remains readable");
    assert_eq!(
        migrated,
        (1, "Room transcript remains historical".to_owned())
    );
    let room_sources: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM agent_chat_source_ref
         WHERE source_type = 'room' AND source_id = 'room-history-account'",
    )
    .fetch_one(&pool)
    .await
    .expect("Room provenance remains readable");
    assert_eq!(room_sources, 1);
}

#[tokio::test]
async fn v118_preserves_history_and_revokes_only_binding_only_memberships() {
    let temp = tempfile::tempdir().expect("temp dir");
    let migrations = temp.path().join("migrations");
    fs::create_dir_all(&migrations).expect("migration dir");
    copy_migrations_through(117, &migrations);
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("in-memory database");
    run_migrations_from(&pool, &migrations)
        .await
        .expect("schema through V117");

    let now = now_rfc3339();
    for (id, email) in [
        ("pr11-owner", "owner@example.test"),
        ("pr11-agent-owner", "agent-owner@example.test"),
        ("pr11-member-owner", "member-owner@example.test"),
    ] {
        sqlx::query(
            "INSERT INTO user (id, email, password_hash, created_at, updated_at)
             VALUES (?, ?, 'fixture', ?, ?)",
        )
        .bind(id)
        .bind(email)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("fixture user");
    }
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, owner_id, created_at, updated_at)
         VALUES ('pr11-fixture-project', 'PR11 fixture', '{}', '{}', 'pr11-owner', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("fixture Project");
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, owner_id, created_at, updated_at)
         VALUES ('pr11-binding-project', 'Binding-only fixture', '{}', '{}', 'pr11-owner', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("binding-only Project");
    sqlx::query(
        "INSERT INTO project_member (id, project_id, user_id, role, created_at, updated_at)
         VALUES ('pr11-member-row', 'pr11-fixture-project', 'pr11-member-owner', 'member', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("independent account Project eligibility");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, task_type, status, created_at, updated_at)
         VALUES ('pr11-real-task', 'pr11-fixture-project', NULL, 'Existing Task', 'implementation', 'todo', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("existing Task");
    sqlx::query(
        "INSERT INTO task (id, project_id, repo_id, title, task_type, status, created_at, updated_at)
         VALUES ('pr11-binding-task', 'pr11-binding-project', NULL, 'Binding-scoped Task', 'implementation', 'todo', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("binding-scoped Task");

    for (id, owner_id, visibility, profile_id) in [
        (
            "pr11-binding-only",
            "pr11-agent-owner",
            "account",
            "pr11-profile-binding",
        ),
        (
            "pr11-global",
            "pr11-agent-owner",
            "global",
            "pr11-profile-global",
        ),
        (
            "pr11-account-member",
            "pr11-member-owner",
            "account",
            "pr11-profile-member",
        ),
    ] {
        sqlx::query(
            "INSERT INTO agent_identity (id, name, owner_id, visibility, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(id)
        .bind(owner_id)
        .bind(visibility)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("fixture Agent identity");
        sqlx::query(
            "INSERT INTO agent_profile (id, identity_id, backend_kind, executor_type, created_at, updated_at)
             VALUES (?, ?, 'cli', 'test', ?, ?)",
        )
        .bind(profile_id)
        .bind(id)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("fixture Agent profile");
        sqlx::query("UPDATE agent_identity SET selected_profile_id = ? WHERE id = ?")
            .bind(profile_id)
            .bind(id)
            .execute(&pool)
            .await
            .expect("selected Agent profile");
    }
    for (id, project_id, identity_id, profile_id) in [
        (
            "pr11-project-binding-global",
            "pr11-fixture-project",
            "pr11-global",
            "pr11-profile-global",
        ),
        (
            "pr11-project-binding-only",
            "pr11-binding-project",
            "pr11-binding-only",
            "pr11-profile-binding",
        ),
    ] {
        sqlx::query(
            "UPDATE project_agent_binding SET state = 'replaced',
             replacement_reason = 'fixture replaces setup row' WHERE project_id = ?",
        )
        .bind(project_id)
        .execute(&pool)
        .await
        .expect("replace setup-only Project binding");
        sqlx::query(
            "INSERT INTO project_agent_binding (id, project_id, identity_id, profile_id, state, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'active', ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(identity_id)
        .bind(profile_id)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("historical Project Agent binding");
    }
    sqlx::query(
        "INSERT INTO account_main_agent_binding
         (id, account_id, identity_id, profile_id, state, created_at, updated_at)
         VALUES ('pr11-main-binding', 'pr11-agent-owner', 'pr11-binding-only',
                 'pr11-profile-binding', 'active', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Main binding");

    for (id, task_id, role) in [
        ("pr11-implementer", "pr11-real-task", "implementer"),
        (
            "pr11-binding-implementer",
            "pr11-binding-task",
            "implementer",
        ),
        ("pr11-binding-reviewer", "pr11-binding-task", "reviewer"),
    ] {
        sqlx::query(
            "INSERT INTO task_role (id, task_id, role, coordination_mode, policy_json, created_at, updated_at)
             VALUES (?, ?, ?, 'collaborative', '{}', ?, ?)",
        )
        .bind(id)
        .bind(task_id)
        .bind(role)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("TaskRole");
    }
    for (id, role_id, actor_id, status) in [
        (
            "pr11-binding-active",
            "pr11-binding-implementer",
            "pr11-binding-only",
            "active",
        ),
        (
            "pr11-binding-suspended",
            "pr11-binding-reviewer",
            "pr11-binding-only",
            "suspended",
        ),
        (
            "pr11-global-role",
            "pr11-implementer",
            "pr11-global",
            "active",
        ),
        (
            "pr11-member-role",
            "pr11-implementer",
            "pr11-account-member",
            "active",
        ),
    ] {
        sqlx::query(
            "INSERT INTO role_membership
             (id, task_role_id, actor_kind, actor_id, status, created_at, updated_at)
             VALUES (?, ?, 'agent', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(role_id)
        .bind(actor_id)
        .bind(status)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("Task Role membership");
    }
    sqlx::query(
        "INSERT INTO role_membership
         (id, task_role_id, actor_kind, actor_id, status, created_at, updated_at)
         VALUES ('pr11-binding-task-owner-role', 'pr11-binding-implementer',
                 'human', 'pr11-owner', 'active', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("eligible human remains alongside revoked Agent");
    sqlx::query(
        "UPDATE task SET assignee_type = 'agent', assignee_id = 'pr11-binding-only'
         WHERE id = 'pr11-binding-task'",
    )
    .execute(&pool)
    .await
    .expect("historical singleton Task projection");
    sqlx::query(
        "INSERT INTO task_role_assignment
         (id, task_id, role_name, assignee_type, assignee_id, created_at, updated_at)
         VALUES ('pr11-binding-assignment', 'pr11-binding-task', 'implementer',
                 'agent', 'pr11-binding-only', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical compatibility role projection");

    let main_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'account_main' AND account_id = 'pr11-owner'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical Main Chat");
    seed_legacy_project_os_and_release(&pool, &now, &main_chat_id, "pr11-global").await;
    sqlx::query(
        "INSERT INTO agent_chat_message
         (id, chat_id, sequence, author_type, author_id, content, status, correlation_id, source_type, created_at)
         VALUES ('pr11-pending-message', ?, 1, 'user', 'pr11-owner', 'pending request',
                 'complete', 'pr11-pending-correlation', 'native', ?)",
    )
    .bind(&main_chat_id)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical user chat message");
    sqlx::query(
        "INSERT INTO agent_chat_turn_job
         (id, chat_id, triggering_message_id, responder_identity_id, profile_id,
          canonical_scope_type, canonical_scope_id, status, dedupe_key, correlation_id, created_at, updated_at)
         VALUES ('pr11-pending-turn', ?, 'pr11-pending-message', 'pr11-binding-only',
                 'pr11-profile-binding', 'agent_chat', ?, 'queued', 'pr11-pending-turn-key',
                 'pr11-pending-correlation', ?, ?)",
    )
    .bind(&main_chat_id)
    .bind(&main_chat_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical queued turn");
    for (message_id, sequence, status, lease_owner) in [
        (
            "pr11-leased-message",
            2_i64,
            "leased",
            Some("legacy-worker"),
        ),
        ("pr11-retry-message", 3_i64, "retry_wait", None),
    ] {
        sqlx::query(
            "INSERT INTO agent_chat_message
             (id, chat_id, sequence, author_type, author_id, content, status,
              correlation_id, source_type, created_at)
             VALUES (?, ?, ?, 'user', 'pr11-owner', 'pending legacy turn',
                     'complete', ?, 'native', ?)",
        )
        .bind(message_id)
        .bind(&main_chat_id)
        .bind(sequence)
        .bind(format!("{message_id}-correlation"))
        .bind(&now)
        .execute(&pool)
        .await
        .expect("legacy pending turn message");
        let job_id = if status == "leased" {
            "pr11-leased-turn"
        } else {
            "pr11-retry-turn"
        };
        sqlx::query(
            "INSERT INTO agent_chat_turn_job
             (id, chat_id, triggering_message_id, responder_identity_id, profile_id,
              canonical_scope_type, canonical_scope_id, status, dedupe_key,
              lease_owner, leased_until, attempt_count, next_attempt_at,
              correlation_id, created_at, updated_at)
             VALUES (?, ?, ?, 'pr11-global', 'pr11-profile-global', 'agent_chat',
                     ?, ?, ?, ?, ?, 1, ?, ?, ?, ?)",
        )
        .bind(job_id)
        .bind(&main_chat_id)
        .bind(message_id)
        .bind(&main_chat_id)
        .bind(status)
        .bind(format!("{job_id}-key"))
        .bind(lease_owner)
        .bind(lease_owner.map(|_| "2000-01-01T00:00:00Z"))
        .bind((status == "retry_wait").then_some("2000-01-01T00:00:00Z"))
        .bind(format!("{job_id}-correlation"))
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("legacy pending turn job");
    }

    let migration_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    run_migrations_from(&pool, migration_dir)
        .await
        .expect("apply V118 to populated V117 database");
    let issues_before_restart: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pr11_vertical_migration_issue")
            .fetch_one(&pool)
            .await
            .expect("migration reconciliation receipts");
    let migration_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    run_migrations_from(&pool, migration_dir)
        .await
        .expect("restart skips the already committed V118 migration");
    let issues_after_restart: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pr11_vertical_migration_issue")
            .fetch_one(&pool)
            .await
            .expect("restart leaves reconciliation receipts stable");
    assert_eq!(issues_after_restart, issues_before_restart);

    for (id, status) in [
        ("pr11-binding-active", "ended"),
        ("pr11-binding-suspended", "ended"),
        ("pr11-global-role", "active"),
        ("pr11-member-role", "active"),
    ] {
        let actual: String = sqlx::query_scalar("SELECT status FROM role_membership WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .expect("membership remains auditable");
        assert_eq!(actual, status, "membership {id}");
    }
    let task_projection: (Option<String>, Option<String>, i64) = sqlx::query_as(
        "SELECT assignee_type, assignee_id, version
         FROM task WHERE id = 'pr11-binding-task'",
    )
    .fetch_one(&pool)
    .await
    .expect("Task compatibility projection remains valid");
    assert_eq!(
        task_projection,
        (None, None, 2),
        "no Actor is assigned as replacement and Task version advances"
    );
    let role_projection: (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT assignee_type, assignee_id FROM task_role_assignment
         WHERE id = 'pr11-binding-assignment'",
    )
    .fetch_one(&pool)
    .await
    .expect("TaskRole compatibility projection remains valid");
    assert_eq!(
        role_projection,
        (None, None),
        "legacy singleton is cleared, not reassigned"
    );
    let pending_job_status: String =
        sqlx::query_scalar("SELECT status FROM agent_chat_turn_job WHERE id = 'pr11-pending-turn'")
            .fetch_one(&pool)
            .await
            .expect("pending turn remains historical");
    assert_eq!(pending_job_status, "queued");
    let unclaimable = sqlx::query(
        "UPDATE agent_chat_turn_job SET status = 'leased', lease_owner = 'restart',
         leased_until = ?, updated_at = ? WHERE id = 'pr11-pending-turn'",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(
        unclaimable.is_err(),
        "legacy turn cannot be claimed after upgrade"
    );
    let leased_turn_status: String =
        sqlx::query_scalar("SELECT status FROM agent_chat_turn_job WHERE id = 'pr11-leased-turn'")
            .fetch_one(&pool)
            .await
            .expect("pre-cutover lease remains historical");
    assert_eq!(leased_turn_status, "leased");
    let expired_lease_reclaim = sqlx::query(
        "UPDATE agent_chat_turn_job SET lease_owner = 'restart',
         leased_until = ?, updated_at = ? WHERE id = 'pr11-leased-turn'",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await;
    assert!(
        expired_lease_reclaim.is_err(),
        "a leased legacy turn cannot be reclaimed after upgrade"
    );

    let historical: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pr11_vertical_migration_issue
         WHERE source_kind = 'agent_chat' AND disposition = 'historical_only'",
    )
    .fetch_one(&pool)
    .await
    .expect("history issue receipt");
    assert_eq!(
        historical, 5,
        "three user Main Chats and both Project Chats are recorded"
    );
    let revoked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pr11_vertical_migration_issue
         WHERE source_kind = 'role_membership' AND disposition = 'historical_only'",
    )
    .fetch_one(&pool)
    .await
    .expect("membership revocation receipt");
    assert_eq!(
        revoked, 2,
        "active and suspended binding-only memberships are audited"
    );
    let linked_commitment: (String, String) = sqlx::query_as(
        "SELECT disposition, details_json FROM pr11_vertical_migration_issue
         WHERE source_kind = 'agent_commitment' AND source_id = 'pr11-task-commitment'",
    )
    .fetch_one(&pool)
    .await
    .expect("linked commitment audit");
    assert_eq!(linked_commitment.0, "already_represented");
    assert!(linked_commitment.1.contains("pr11-real-task"));
    let legacy_action_status: String = sqlx::query_scalar(
        "SELECT status FROM agent_action WHERE id = 'pr11-approved-unexecuted-action'",
    )
    .fetch_one(&pool)
    .await
    .expect("approved action remains historical");
    assert_eq!(legacy_action_status, "approved");
    let pending_action_status: String =
        sqlx::query_scalar("SELECT status FROM agent_action WHERE id = 'pr11-pending-action'")
            .fetch_one(&pool)
            .await
            .expect("pending action remains historical");
    assert_eq!(pending_action_status, "pending_approval");
    assert!(sqlx::query(
        "UPDATE agent_action SET status = 'executing'
         WHERE id IN ('pr11-pending-action', 'pr11-approved-unexecuted-action')",
    )
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "INSERT INTO agent_action_execution
         (id, action_id, attempt, status, executed_by_type, executed_by_id,
          idempotency_key, created_at)
         VALUES ('pr11-replay-execution', 'pr11-approved-unexecuted-action', 1,
                 'started', 'system', 'restart', 'pr11-replay-execution-key', ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE agent_commitment SET status = 'completed'
         WHERE id = 'pr11-task-commitment'",
    )
    .execute(&pool)
    .await
    .is_err());
    let task_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM task WHERE project_id = 'pr11-fixture-project'")
            .fetch_one(&pool)
            .await
            .expect("no fabricated Task");
    assert_eq!(task_count, 1);
    let task_artifacts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM artifact WHERE task_id = 'pr11-real-task'")
            .fetch_one(&pool)
            .await
            .expect("no invented Artifact");
    let task_proposals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM proposal WHERE task_id = 'pr11-real-task'")
            .fetch_one(&pool)
            .await
            .expect("no invented Proposal");
    let task_decisions: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM decision WHERE task_id = 'pr11-real-task'")
            .fetch_one(&pool)
            .await
            .expect("no invented Decision");
    assert_eq!((task_artifacts, task_proposals, task_decisions), (0, 0, 0));
    let old_document: (String, String) = sqlx::query_as(
        "SELECT d.title, i.disposition FROM project_document d
         JOIN pr11_vertical_migration_issue i
           ON i.source_kind = 'project_document' AND i.source_id = d.id
         WHERE d.id = 'pr11-document'",
    )
    .fetch_one(&pool)
    .await
    .expect("ambiguous Document remains historical without Artifact");
    assert_eq!(
        old_document,
        (
            "Historical architecture".to_owned(),
            "historical_only".to_owned()
        )
    );
    let event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE dedupe_key = 'pr11:project-agent-scope:pr11-binding-task'",
    )
    .fetch_one(&pool)
    .await
    .expect("scope-change event");
    assert_eq!(event_count, 1, "Task event is idempotently recorded");

    let genesis_lifecycle: String = sqlx::query_scalar(
        "SELECT lifecycle FROM product_genesis_session WHERE id = 'pr11-active-genesis'",
    )
    .fetch_one(&pool)
    .await
    .expect("active Genesis remains historical");
    assert_eq!(genesis_lifecycle, "discovering");
    let genesis_update = sqlx::query(
        "UPDATE product_genesis_session SET lifecycle = 'cancelled' WHERE id = 'pr11-active-genesis'",
    )
    .execute(&pool)
    .await;
    assert!(
        genesis_update.is_err(),
        "Genesis cannot continue after cutover"
    );
    let genesis_lifecycles: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, lifecycle FROM product_genesis_session
         WHERE id IN ('pr11-active-genesis', 'pr11-ready-genesis',
                      'pr11-handed-off-genesis', 'pr11-cancelled-genesis')
         ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("Genesis lifecycle history remains readable");
    assert_eq!(
        genesis_lifecycles,
        vec![
            ("pr11-active-genesis".to_owned(), "discovering".to_owned()),
            ("pr11-cancelled-genesis".to_owned(), "cancelled".to_owned()),
            (
                "pr11-handed-off-genesis".to_owned(),
                "handed_off".to_owned()
            ),
            (
                "pr11-ready-genesis".to_owned(),
                "ready_for_project".to_owned()
            ),
        ]
    );
    let projects_after_genesis: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project")
        .fetch_one(&pool)
        .await
        .expect("Genesis does not synthesize Projects");
    assert_eq!(projects_after_genesis, 2);

    let release_snapshot: (String, String) = sqlx::query_as(
        "SELECT r.snapshot_digest, p.storage_key
         FROM project_release r
         JOIN project_release_media_pin pin ON pin.release_id = r.id
         JOIN media_asset p ON p.id = pin.asset_id
         WHERE r.id = 'pr11-release' AND pin.id = 'pr11-release-pin'",
    )
    .fetch_one(&pool)
    .await
    .expect("completed release and media pin remain readable");
    assert_eq!(release_snapshot.0, "pr11-release-snapshot");
    assert_eq!(release_snapshot.1, "historical/pr11-release-proof.png");
    let release_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pr11_vertical_migration_issue
         WHERE source_kind = 'project_release' AND source_id = 'pr11-release'
           AND disposition = 'historical_only'",
    )
    .fetch_one(&pool)
    .await
    .expect("release historical receipt");
    assert_eq!(release_count, 1);
    let task_governance: i64 = sqlx::query_scalar(
        "SELECT runnable FROM project_task_governance WHERE task_id = 'pr11-real-task'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical runnable projection remains readable");
    assert_eq!(
        task_governance, 1,
        "legacy projection is preserved as history"
    );
    let governance_write = sqlx::query(
        "UPDATE project_task_governance SET runnable = 0 WHERE task_id = 'pr11-real-task'",
    )
    .execute(&pool)
    .await;
    assert!(
        governance_write.is_err(),
        "Project Task Governance is frozen"
    );
}

async fn seed_legacy_project_os_and_release(
    pool: &sqlx::SqlitePool,
    now: &str,
    main_chat_id: &str,
    agent_id: &str,
) {
    sqlx::query(
        "INSERT INTO product_genesis_session
         (id, account_id, main_chat_id, prompt_revision, prompt_body, maturity,
          initial_idea, lifecycle, source_message_ids_json, version, created_at, updated_at)
         VALUES ('pr11-active-genesis', 'pr11-owner', ?, 'genesis-v1', 'preserve me',
                 'mvp', 'historical idea', 'discovering', '[]', 1, ?, ?)",
    )
    .bind(main_chat_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("active historical Genesis");
    let other_main_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'account_main'
         AND account_id = 'pr11-agent-owner'",
    )
    .fetch_one(pool)
    .await
    .expect("second account Main Chat");
    sqlx::query(
        "INSERT INTO product_genesis_session
         (id, account_id, main_chat_id, prompt_revision, prompt_body, maturity,
          initial_idea, lifecycle, source_message_ids_json, version, created_at, updated_at)
         VALUES ('pr11-ready-genesis', 'pr11-agent-owner', ?, 'genesis-v1', 'ready history',
                 'mvp', 'ready idea', 'ready_for_project', '[]', 1, ?, ?)",
    )
    .bind(&other_main_chat_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("ready-for-Project Genesis history");
    let project_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'project'
         AND project_id = 'pr11-fixture-project'",
    )
    .fetch_one(pool)
    .await
    .expect("historical Project Chat");
    sqlx::query(
        "INSERT INTO agent_handoff
         (id, source_chat_id, target_chat_id, content, correlation_id,
          dedupe_key, created_at, updated_at)
         VALUES ('pr11-genesis-handoff', ?, ?, 'historical handoff',
                 'pr11-genesis-handoff-correlation', 'pr11-genesis-handoff-key', ?, ?)",
    )
    .bind(main_chat_id)
    .bind(&project_chat_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Main-to-Project handoff");
    sqlx::query(
        "INSERT INTO product_genesis_session
         (id, account_id, main_chat_id, prompt_revision, prompt_body, maturity,
          initial_idea, lifecycle, source_message_ids_json, project_id, handoff_id,
          version, created_at, updated_at)
         VALUES ('pr11-handed-off-genesis', 'pr11-owner', ?, 'genesis-v1', 'handed off history',
                 'mvp', 'handed off idea', 'handed_off', '[]',
                 'pr11-fixture-project', 'pr11-genesis-handoff', 1, ?, ?)",
    )
    .bind(main_chat_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("handed-off Genesis history");
    sqlx::query(
        "INSERT INTO product_genesis_session
         (id, account_id, main_chat_id, prompt_revision, prompt_body, maturity,
          initial_idea, lifecycle, source_message_ids_json, version, created_at, updated_at)
         VALUES ('pr11-cancelled-genesis', 'pr11-owner', ?, 'genesis-v1', 'cancelled history',
                 'mvp', 'cancelled idea', 'cancelled', '[]', 1, ?, ?)",
    )
    .bind(main_chat_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("cancelled Genesis history");
    sqlx::query(
        "INSERT INTO project_charter
         (id, account_id, genesis_session_id, project_id, project_mode, maturity,
          lifecycle, created_at, updated_at)
         VALUES ('pr11-charter', 'pr11-owner', 'pr11-active-genesis',
                 'pr11-fixture-project', 'standard', 'mvp', 'attached', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Charter");
    sqlx::query(
        "INSERT INTO project_charter_revision
         (id, charter_id, revision, lifecycle, schema_version, render_version,
          content_json, rendered_view, author_type, author_id, content_digest,
          rendered_digest, created_at)
         VALUES ('pr11-charter-r1', 'pr11-charter', 1, 'approved', 'test', 'test',
                 '{}', 'historical Charter', 'user', 'pr11-owner', 'charter-digest',
                 'charter-render-digest', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Charter revision");
    sqlx::query(
        "UPDATE project_charter
         SET current_approved_revision_id = 'pr11-charter-r1'
         WHERE id = 'pr11-charter'",
    )
    .execute(pool)
    .await
    .expect("historical Charter approved revision");
    sqlx::query(
        "UPDATE project
         SET charter_status = 'charter_backed', charter_setup_required = 0,
             current_charter_id = 'pr11-charter',
             current_charter_revision_id = 'pr11-charter-r1',
             current_charter_version = 1
         WHERE id = 'pr11-fixture-project'",
    )
    .execute(pool)
    .await
    .expect("historical Charter-backed Project");
    sqlx::query(
        "INSERT INTO project_document
         (id, project_id, kind, title, lifecycle, created_at, updated_at)
         VALUES ('pr11-document', 'pr11-fixture-project', 'architecture',
                 'Historical architecture', 'approved', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Project Document");
    sqlx::query(
        "INSERT INTO project_execution_baseline
         (id, project_id, lifecycle, current_revision_id, created_at, updated_at)
         VALUES ('pr11-baseline', 'pr11-fixture-project', 'active', NULL, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical active baseline");
    sqlx::query(
        "INSERT INTO project_execution_baseline_revision
         (id, baseline_id, revision, lifecycle, charter_revision_id,
          release_policy_revision, release_policy_digest, schema_version,
          render_version, rendered_view, content_digest, rendered_digest, created_at)
         VALUES ('pr11-baseline-r1', 'pr11-baseline', 1, 'approved',
                 'pr11-charter-r1', 'policy-v1', 'policy-digest', 'test',
                 'test', 'historical baseline', 'baseline-digest',
                 'baseline-render-digest', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical baseline revision");
    sqlx::query(
        "UPDATE project_execution_baseline
         SET current_revision_id = 'pr11-baseline-r1'
         WHERE id = 'pr11-baseline'",
    )
    .execute(pool)
    .await
    .expect("historical baseline current revision");
    sqlx::query(
        "INSERT INTO project_execution_baseline_approval
         (id, baseline_id, revision_id, expected_project_version, principal_type,
          principal_id, authorization_basis, authorization_action, explicit_event,
          authorization_occurred_at, content_digest, rendered_digest,
          idempotency_key, created_at, updated_at)
         VALUES ('pr11-baseline-approval', 'pr11-baseline', 'pr11-baseline-r1', 1,
                 'user', 'pr11-owner', 'historical approval',
                 'project.execution_baseline.approve', 'approve exact baseline',
                 '2026-01-01T00:00:00Z', 'baseline-digest',
                 'baseline-render-digest', 'pr11-baseline-approval-key', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical baseline approval");
    sqlx::query(
        "INSERT INTO project_task_governance
         (task_id, project_id, charter_revision_id, baseline_id, baseline_revision_id,
          runnable, provenance_json, created_at, updated_at)
         VALUES ('pr11-real-task', 'pr11-fixture-project', 'pr11-charter-r1',
                 'pr11-baseline', 'pr11-baseline-r1', 1, '{}', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Task Governance projection");
    sqlx::query(
        "INSERT INTO project_milestone
         (id, project_id, milestone_sequence, milestone_key, lifecycle, created_at, updated_at)
         VALUES ('pr11-milestone', 'pr11-fixture-project', 1, 'M001', 'released', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical milestone");
    sqlx::query(
        "INSERT INTO project_milestone_revision
         (id, milestone_id, revision, base_revision, lifecycle, outcome,
          included_scope_json, excluded_scope_json, document_revisions_json,
          task_selection_json, dependencies_json, risks_json, acceptance_checks_json,
          evidence_requirements_json, known_issues_json, change_summary, schema_version,
          render_version, rendered_view, content_digest, rendered_digest,
          author_type, source_refs_json, created_at)
         VALUES ('pr11-milestone-r1', 'pr11-milestone', 1, 0, 'approved', 'historical outcome',
                 '[]', '[]', '[]', '[]', '[]', '[]', '[]', '[]', '[]', '',
                 'test', 'test', 'historical milestone', 'milestone-digest',
                 'milestone-render-digest', 'user', '[]', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical milestone revision");
    sqlx::query(
        "INSERT INTO project_readiness_snapshot
         (id, project_id, milestone_id, definition_revision_id, baseline_id,
          baseline_revision_id, baseline_digest, release_policy_revision,
          release_policy_digest, event_watermark, outcome, computing_policy_revision,
          readiness_digest, principal_type, principal_id, authorization_basis,
          authorization_action, authorization_occurred_at, expected_milestone_version,
          explicit_event, idempotency_key, created_at)
         VALUES ('pr11-readiness', 'pr11-fixture-project', 'pr11-milestone',
                 'pr11-milestone-r1', 'pr11-baseline', 'pr11-baseline-r1',
                 'baseline-digest', 'policy-v1', 'policy-digest', 'watermark', 'ready',
                 'test', 'readiness-digest', 'user', 'pr11-owner', 'test',
                 'readiness.evaluate', '2026-01-01T00:00:00Z', 1, 'event',
                 'pr11-readiness-key', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical readiness snapshot");
    sqlx::query(
        "INSERT INTO project_release
         (id, project_id, milestone_id, release_sequence, release_revision,
          release_identifier, milestone_revision_id, readiness_snapshot_id,
          readiness_digest, baseline_id, baseline_revision_id, baseline_digest,
          release_policy_revision, release_policy_digest,
          releasing_principal_type, releasing_principal_id, authorization_basis,
          authorization_action, authorization_occurred_at, explicit_event,
          schema_version, snapshot_digest, idempotency_key, created_at)
         VALUES ('pr11-release', 'pr11-fixture-project', 'pr11-milestone', 1, 1,
                 'M001-r1', 'pr11-milestone-r1', 'pr11-readiness',
                 'readiness-digest', 'pr11-baseline', 'pr11-baseline-r1',
                 'baseline-digest', 'policy-v1', 'policy-digest', 'user',
                 'pr11-owner', 'test', 'release.create', '2026-01-01T00:00:00Z',
                 'release-event', 'test', 'pr11-release-snapshot', 'release-key', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("historical completed Release");
    sqlx::query(
        "INSERT INTO media_asset
         (id, project_id, display_filename, content_type, byte_size, storage_key,
          checksum, created_at, updated_at)
         VALUES ('pr11-release-asset', 'pr11-fixture-project', 'proof.png',
                 'image/png', 4, 'historical/pr11-release-proof.png', 'asset-checksum', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("shared media asset");
    sqlx::query(
        "INSERT INTO project_release_media_pin
         (id, project_id, release_id, asset_id, asset_checksum, attachment_digest,
          availability, pin_digest, created_at)
         VALUES ('pr11-release-pin', 'pr11-fixture-project', 'pr11-release',
                 'pr11-release-asset', 'asset-checksum', 'attachment-digest',
                 'available', 'pin-digest', ?)",
    )
    .bind(now)
    .execute(pool)
    .await
    .expect("immutable release media pin");
    sqlx::query(
        "INSERT INTO agent_action
         (id, actor_identity_id, scope_type, scope_id, operation, payload_json,
          payload_hash, dedupe_key, correlation_id, requested_permission,
          policy_result, status, target_type, target_id, outcome_json, created_at, updated_at)
         VALUES ('pr11-pending-action', ?, 'project', 'pr11-fixture-project',
                 'task.propose', '{}', 'pending-action-hash', 'pending-action',
                 'pending-correlation', 'propose_task', 'approval_required',
                 'pending_approval', NULL, NULL, NULL, ?, ?)",
    )
    .bind(agent_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("pending AgentAction");
    sqlx::query(
        "INSERT INTO agent_action
         (id, actor_identity_id, scope_type, scope_id, operation, payload_json,
          payload_hash, dedupe_key, correlation_id, requested_permission,
          policy_result, status, target_type, target_id, outcome_json, created_at, updated_at)
         VALUES ('pr11-approved-unexecuted-action', ?, 'project', 'pr11-fixture-project',
                 'task.propose', '{}', 'approved-action-hash', 'approved-action',
                 'approved-correlation', 'propose_task', 'allowed', 'approved',
                 NULL, NULL, NULL, ?, ?)",
    )
    .bind(agent_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("approved but unexecuted AgentAction");
    sqlx::query(
        "INSERT INTO agent_action
         (id, actor_identity_id, scope_type, scope_id, operation, payload_json,
          payload_hash, dedupe_key, correlation_id, requested_permission,
          policy_result, status, target_type, target_id, outcome_json, created_at, updated_at)
         VALUES ('pr11-task-propose-action', ?, 'task', 'pr11-real-task', 'task.propose',
                 '{}', 'task-propose-hash', 'task-propose-key', 'task-propose-correlation',
                 'propose_task', 'allowed', 'executed', 'task', 'pr11-real-task',
                 '{\"task_id\":\"pr11-real-task\"}', ?, ?)",
    )
    .bind(agent_id)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("approved but unexecuted AgentAction");
    for (id, scope_type, scope_id, task_id) in [
        (
            "pr11-task-commitment",
            "task",
            "pr11-real-task",
            Some("pr11-real-task"),
        ),
        (
            "pr11-unlinked-commitment",
            "project",
            "pr11-fixture-project",
            None,
        ),
    ] {
        sqlx::query(
            "INSERT INTO agent_commitment
             (id, owner_identity_id, scope_type, scope_id, title, status,
              correlation_id, originating_task_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'historical obligation', 'open', ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(agent_id)
        .bind(scope_type)
        .bind(scope_id)
        .bind(format!("{id}-correlation"))
        .bind(task_id)
        .bind(now)
        .bind(now)
        .execute(pool)
        .await
        .expect("historical Commitment");
    }
}
