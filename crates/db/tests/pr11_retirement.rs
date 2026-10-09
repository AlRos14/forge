use db::{
    create_sqlite_pool, now_rfc3339, run_migrations, run_migrations_from, AgentRepo, AgentStatus,
    CreateAgent, CreateWorkspaceLease, WorkspaceLeaseRepo,
};
use sqlx::SqlitePool;
use std::{collections::BTreeSet, fs, path::Path};

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

async fn genesis_rows(pool: &SqlitePool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT json_object(
            'id', id, 'account_id', account_id, 'main_chat_id', main_chat_id,
            'prompt_revision', prompt_revision, 'prompt_body', prompt_body,
            'maturity', maturity, 'initial_idea', initial_idea, 'lifecycle', lifecycle,
            'source_message_ids_json', source_message_ids_json,
            'preferred_project_agent_identity_id', preferred_project_agent_identity_id,
            'project_id', project_id, 'handoff_id', handoff_id,
            'failure_reason', failure_reason, 'version', version,
            'created_at', created_at, 'updated_at', updated_at,
            'charter_id', charter_id, 'charter_revision_id', charter_revision_id,
            'charter_approval_id', charter_approval_id, 'charter_version', charter_version
        ) FROM product_genesis_session ORDER BY id",
    )
    .fetch_all(pool)
    .await
    .expect("Genesis history snapshot")
}

async fn sqlite_schema_snapshot(pool: &SqlitePool) -> Vec<(String, String, String, String)> {
    sqlx::query_as(
        "SELECT type, name, tbl_name, sql FROM sqlite_master
         WHERE sql IS NOT NULL ORDER BY type, name",
    )
    .fetch_all(pool)
    .await
    .expect("SQLite schema snapshot")
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
        "INSERT INTO domain_event
         (id, event_type, entity_type, entity_id, actor_type, actor_id,
          scope_type, scope_id, correlation_id, payload_json, created_at)
         VALUES ('pr11-project-history-event', 'project.created', 'project',
                 'pr11-fixture-project', 'user', 'pr11-owner', 'project',
                 'pr11-fixture-project', 'pr11-project-history-correlation', '{}', ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Project domain event");
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
        .expect("apply PR11 migrations through V119 to populated V117 database");
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

    let db = db::SqliteDb::new(pool.clone());
    db::ProjectRepo::delete(&db, "pr11-fixture-project")
        .await
        .expect("Project deletion with historical PR11 rows succeeds");
    let genesis_after_delete: (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT lifecycle, project_id, handoff_id, charter_id,
                charter_revision_id, charter_approval_id
         FROM product_genesis_session WHERE id = 'pr11-handed-off-genesis'",
    )
    .fetch_one(&pool)
    .await
    .expect("handed-off Genesis remains historical");
    assert_eq!(
        genesis_after_delete,
        ("handed_off".to_owned(), None, None, None, None, None),
        "FK cleanup preserves Genesis lifecycle and clears only deleted references"
    );
    let commitment_after_delete: (String, String, Option<String>) = sqlx::query_as(
        "SELECT title, status, originating_task_id
         FROM agent_commitment WHERE id = 'pr11-task-commitment'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical Task commitment remains after Project teardown");
    assert_eq!(
        commitment_after_delete,
        ("historical obligation".to_owned(), "open".to_owned(), None)
    );
    let remaining_project_os: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM project_charter WHERE project_id = 'pr11-fixture-project')
              + (SELECT COUNT(*) FROM project_execution_baseline WHERE project_id = 'pr11-fixture-project')
              + (SELECT COUNT(*) FROM project_milestone WHERE project_id = 'pr11-fixture-project')
              + (SELECT COUNT(*) FROM project_release WHERE project_id = 'pr11-fixture-project')
              + (SELECT COUNT(*) FROM project_release_media_pin WHERE project_id = 'pr11-fixture-project')",
    )
    .fetch_one(&pool)
    .await
    .expect("Project OS teardown count");
    assert_eq!(
        remaining_project_os, 0,
        "deletion follows existing cascade contract"
    );
    let guard_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_deletion_guard")
        .fetch_one(&pool)
        .await
        .expect("Project deletion guard cleanup");
    assert_eq!(guard_rows, 0, "guard is removed before commit");
    let domain_event_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM domain_event WHERE id = 'pr11-project-history-event'",
    )
    .fetch_one(&pool)
    .await
    .expect("domain-event history survives Project deletion");
    assert_eq!(domain_event_count, 1);
}

#[tokio::test]
async fn v119_applies_from_pristine_v118() {
    let temp = tempfile::tempdir().expect("temp dir");
    let migrations = temp.path().join("migrations");
    fs::create_dir_all(&migrations).expect("migration dir");
    copy_migrations_through(118, &migrations);
    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("in-memory database");
    run_migrations_from(&pool, &migrations)
        .await
        .expect("schema through pristine V118");

    copy_migrations_through(119, &migrations);
    run_migrations_from(&pool, &migrations)
        .await
        .expect("V119 applies from pristine V118");

    let receipt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration WHERE version = 119")
        .fetch_one(&pool)
        .await
        .expect("V119 receipt");
    assert_eq!(receipt, 1);
    let exact_fences: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'
         AND name IN ('pr11_retired_update_product_genesis_session',
                      'pr11_retired_update_project_charter')",
    )
    .fetch_one(&pool)
    .await
    .expect("exact Genesis and Charter fences");
    assert_eq!(exact_fences, 2);
    let pending_fences: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger'
         AND name IN ('pr11_v119_product_genesis_update_pending',
                      'pr11_v119_project_charter_update_pending')",
    )
    .fetch_one(&pool)
    .await
    .expect("temporary migration fences removed");
    assert_eq!(pending_fences, 0);
    let fk_errors: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("foreign_key_check after pristine V119");
    assert!(fk_errors.is_empty(), "V119 FK check: {fk_errors:?}");
}

#[tokio::test]
async fn v119_allows_only_exact_task_and_binding_fk_cleanup() {
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
    sqlx::query(
        "INSERT INTO user (id, email, password_hash, created_at, updated_at)
         VALUES ('fk-owner', 'fk-owner@example.test', 'fixture', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("fixture owner");
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, owner_id, created_at, updated_at)
         VALUES ('fk-project', 'FK teardown', '{}', '{}', 'fk-owner', ?, ?),
                ('fk-keep-project', 'FK history survivor', '{}', '{}', 'fk-owner', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("fixture Project");
    let main_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'account_main' AND account_id = 'fk-owner'",
    )
    .fetch_one(&pool)
    .await
    .expect("account Main Chat");
    let project_chat_id: String = sqlx::query_scalar(
        "SELECT id FROM agent_chat WHERE kind = 'project' AND project_id = 'fk-project'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical Project Chat");
    sqlx::query(
        "INSERT INTO agent_chat_message
         (id, chat_id, sequence, author_type, author_id, content, status,
          correlation_id, source_type, created_at)
         VALUES ('fk-genesis-source', ?, 1, 'user', 'fk-owner', 'source provenance',
                 'complete', 'fk-genesis-source-correlation', 'native', ?)",
    )
    .bind(&main_chat_id)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("Genesis source message");
    sqlx::query(
        "INSERT INTO agent_handoff
         (id, source_chat_id, target_chat_id, content, correlation_id,
          dedupe_key, created_at, updated_at)
         VALUES ('fk-genesis-handoff', ?, ?, 'historical handoff',
                 'fk-genesis-handoff-correlation', 'fk-genesis-handoff-key', ?, ?)",
    )
    .bind(&main_chat_id)
    .bind(&project_chat_id)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Main-to-Project handoff");
    sqlx::query(
        "INSERT INTO agent_identity (id, name, owner_id, visibility, created_at, updated_at)
         VALUES ('fk-agent', 'FK Agent', 'fk-owner', 'global', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("fixture Agent identity");
    sqlx::query(
        "INSERT INTO agent_profile
         (id, identity_id, backend_kind, executor_type, created_at, updated_at)
         VALUES ('fk-profile', 'fk-agent', 'cli', 'test', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("fixture Agent profile");
    sqlx::query(
        "UPDATE agent_identity SET selected_profile_id = 'fk-profile' WHERE id = 'fk-agent'",
    )
    .execute(&pool)
    .await
    .expect("selected Agent profile");
    sqlx::query(
        "INSERT INTO product_genesis_session
         (id, account_id, main_chat_id, prompt_revision, prompt_body, maturity,
          initial_idea, lifecycle, source_message_ids_json,
          preferred_project_agent_identity_id, project_id, handoff_id, version,
          created_at, updated_at)
         VALUES ('fk-genesis', 'fk-owner', ?, 'v7', 'history', 'mvp',
                 'historical idea', 'handed_off', ?,
                 'fk-agent', 'fk-project', 'fk-genesis-handoff', 7, ?, ?)",
    )
    .bind(&main_chat_id)
    .bind(r#"["fk-genesis-source"]"#)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Product Genesis");
    sqlx::query(
        "INSERT INTO project_charter
         (id, account_id, genesis_session_id, project_id, project_mode,
          maturity, lifecycle, created_at, updated_at)
         VALUES ('fk-charter', 'fk-owner', 'fk-genesis', 'fk-project',
                 'standard', 'mvp', 'attached', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Project Charter");
    sqlx::query(
        "INSERT INTO project_charter_revision
         (id, charter_id, revision, lifecycle, schema_version, render_version,
          content_json, rendered_view, author_type, author_id, content_digest,
          rendered_digest, created_at)
         VALUES ('fk-charter-r1', 'fk-charter', 1, 'approved', 'test', 'test',
                 '{}', 'historical Genesis Charter', 'user', 'fk-owner',
                 'fk-charter-digest', 'fk-charter-render-digest', ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("Genesis Charter revision");
    sqlx::query(
        "INSERT INTO project_charter_approval
         (id, approval_type, charter_id, revision_id, content_digest, rendered_digest,
          expected_charter_version, approving_principal_type, approving_principal_id,
          authorization_basis, authorization_action, explicit_event,
          authorization_occurred_at, source_action, idempotency_key, created_at, updated_at)
         VALUES ('fk-genesis-approval', 'project_creation', 'fk-charter', 'fk-charter-r1',
                 'fk-charter-digest', 'fk-charter-render-digest', 1, 'user', 'fk-owner',
                 'historical approval', 'project.create', 'approve Genesis Charter',
                 ?, 'fixture', 'fk-genesis-approval-key', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("Genesis Charter approval");
    sqlx::query(
        "UPDATE product_genesis_session
         SET charter_id = 'fk-charter', charter_revision_id = 'fk-charter-r1',
             charter_approval_id = 'fk-genesis-approval', charter_version = 4
         WHERE id = 'fk-genesis'",
    )
    .execute(&pool)
    .await
    .expect("Genesis Charter provenance");
    sqlx::query(
        "INSERT INTO project_charter
         (id, account_id, project_id, project_mode, maturity, lifecycle, created_at, updated_at)
         VALUES ('fk-keep-charter', 'fk-owner', 'fk-keep-project', 'standard', 'mvp', 'attached', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("surviving Project Charter");
    sqlx::query(
        "INSERT INTO project_charter_revision
         (id, charter_id, revision, lifecycle, schema_version, render_version,
          content_json, rendered_view, author_type, author_id, content_digest,
          rendered_digest, created_at)
         VALUES ('fk-keep-charter-r1', 'fk-keep-charter', 1, 'approved', 'test', 'test',
                 '{}', 'historical approval target', 'user', 'fk-owner',
                 'keep-charter-digest', 'keep-charter-render-digest', ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("surviving Charter revision");
    sqlx::query(
        "INSERT INTO project_charter_approval
         (id, approval_type, charter_id, revision_id, content_digest, rendered_digest,
          expected_charter_version, approving_principal_type, approving_principal_id,
          authorization_basis, authorization_action, explicit_event,
          authorization_occurred_at, source_action, lifecycle, idempotency_key,
          consumed_project_id, consumed_at, created_at, updated_at)
         VALUES ('fk-consumed-approval', 'project_creation', 'fk-keep-charter',
                 'fk-keep-charter-r1', 'keep-charter-digest', 'keep-charter-render-digest',
                 1, 'user', 'fk-owner', 'historical approval', 'project.create',
                 'historical consumed approval', ?, 'fixture', 'consumed',
                 'fk-consumed-approval-key', 'fk-project', ?, ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical approval consumed by the deleting Project");
    sqlx::query(
        "INSERT INTO project_execution_baseline
         (id, project_id, lifecycle, created_at, updated_at)
         VALUES ('fk-baseline', 'fk-project', 'draft', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Execution Baseline");
    sqlx::query(
        "INSERT INTO project_milestone
         (id, project_id, milestone_sequence, milestone_key, lifecycle, created_at, updated_at)
         VALUES ('fk-milestone', 'fk-project', 1, 'M001', 'planned', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Project Milestone");
    for (id, project_id, title) in [
        (
            "fk-task-keep",
            "fk-keep-project",
            "Task keeps historical governance",
        ),
        ("fk-task-delete", "fk-project", "Task removed by teardown"),
    ] {
        sqlx::query(
            "INSERT INTO task
             (id, project_id, repo_id, title, task_type, status, created_at, updated_at)
             VALUES (?, ?, NULL, ?, 'implementation', 'todo', ?, ?)",
        )
        .bind(id)
        .bind(project_id)
        .bind(title)
        .bind(&now)
        .bind(&now)
        .execute(&pool)
        .await
        .expect("fixture Task");
    }
    sqlx::query(
        "INSERT INTO execution (id, task_id, role, status, created_at, updated_at)
         VALUES ('fk-execution', 'fk-task-delete', 'executor', 'completed', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical Execution");
    sqlx::query(
        "UPDATE project_agent_binding
         SET state = 'replaced', replacement_reason = 'historical setup row'
         WHERE project_id = 'fk-project'",
    )
    .execute(&pool)
    .await
    .expect("retire generated setup binding in historical fixture");
    sqlx::query(
        "INSERT INTO project_agent_binding
         (id, project_id, identity_id, profile_id, state, created_at, updated_at)
         VALUES ('fk-binding-b', 'fk-project', 'fk-agent', 'fk-profile', 'active', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("replacement target binding");
    sqlx::query(
        "INSERT INTO project_agent_binding
         (id, project_id, identity_id, profile_id, state, replaced_by_binding_id,
          replacement_reason, created_at, updated_at)
         VALUES ('fk-binding-a', 'fk-keep-project', 'fk-agent', 'fk-profile', 'replaced',
                 'fk-binding-b', 'historical replacement chain', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical replacement chain");
    sqlx::query(
        "INSERT INTO project_task_governance
         (task_id, project_id, replacement_of_task_id, created_at, updated_at)
         VALUES ('fk-task-keep', 'fk-keep-project', 'fk-task-delete', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical replacement governance");
    sqlx::query(
        "INSERT INTO agent_commitment
         (id, owner_identity_id, scope_type, scope_id, title, status,
          correlation_id, originating_task_id, created_at, updated_at)
         VALUES ('fk-commitment', 'fk-agent', 'task', 'fk-task-delete',
                 'historical commitment', 'open', 'fk-commitment-correlation',
                 'fk-task-delete', ?, ?)",
    )
    .bind(&now)
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical commitment");
    sqlx::query(
        "INSERT INTO memory_item
         (id, task_id, execution_id, scope_type, scope_id, source_type,
          kind, title, body, created_at)
         VALUES ('fk-memory', 'fk-task-delete', 'fk-execution', 'task',
                 'fk-task-delete', 'fixture', 'observation',
                 'historical memory', 'body preserved by teardown', ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("historical memory item");

    copy_migrations_through(118, &migrations);
    run_migrations_from(&pool, &migrations)
        .await
        .expect("apply V118 to populated V117 schema");
    let db = db::SqliteDb::new(pool.clone());
    let blocked_by_v118 = db::ProjectRepo::delete(&db, "fk-project")
        .await
        .expect_err("V118 must block deleting the historical handoff");
    assert!(
        format!("{blocked_by_v118:?}").contains("Agent handoffs are immutable"),
        "V118 handoff delete fence blocks Project teardown: {blocked_by_v118:?}"
    );

    let genesis_before_v119 = genesis_rows(&pool).await;
    let v119_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("migrations/V119__allow_fk_cleanup_for_pr11_history.sql");
    let v119_sql = fs::read_to_string(v119_path).expect("read V119 migration");
    let fences_start = v119_sql
        .find("-- Plan PR11 FK maintenance correction.")
        .expect("V119 exact-fence phase marker");
    let rebuild_phase = &v119_sql[..fences_start];
    let mut connection = pool.acquire().await.expect("migration connection");
    sqlx::raw_sql(rebuild_phase)
        .execute(&mut *connection)
        .await
        .expect("simulate V119's durable post-rebuild/pre-fences state");
    drop(connection);
    let receipt_after_partial: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _migration WHERE version = 119")
            .fetch_one(&pool)
            .await
            .expect("partial migration has no receipt");
    assert_eq!(receipt_after_partial, 0);
    let partial_update = sqlx::query(
        "UPDATE product_genesis_session SET prompt_body = 'partial write'
         WHERE id = 'fk-genesis'",
    )
    .execute(&pool)
    .await;
    assert!(
        partial_update.is_err(),
        "partial Genesis rebuild stays fenced"
    );
    let partial_charter_update =
        sqlx::query("UPDATE project_charter SET lifecycle = 'draft' WHERE id = 'fk-charter'")
            .execute(&pool)
            .await;
    assert!(
        partial_charter_update.is_err(),
        "partial Genesis rebuild keeps dependent Charter updates fenced"
    );
    let partial_commitment_update =
        sqlx::query("UPDATE agent_commitment SET status = 'completed' WHERE id = 'fk-commitment'")
            .execute(&pool)
            .await;
    assert!(
        partial_commitment_update.is_err(),
        "V118 retirement fences remain active before V119's exact-fence commit"
    );
    assert_eq!(genesis_rows(&pool).await, genesis_before_v119);

    copy_migrations_through(119, &migrations);
    run_migrations_from(&pool, &migrations)
        .await
        .expect("V119 resumes safely from post-rebuild/pre-fences state");
    let final_genesis_snapshot = genesis_rows(&pool).await;
    assert_eq!(final_genesis_snapshot, genesis_before_v119);
    let final_schema = sqlite_schema_snapshot(&pool).await;
    let receipt_after_apply: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM _migration WHERE version = 119")
            .fetch_one(&pool)
            .await
            .expect("completed V119 receipt");
    assert_eq!(receipt_after_apply, 1);

    sqlx::query("DELETE FROM _migration WHERE version = 119")
        .execute(&pool)
        .await
        .expect("simulate crash after V119 SQL commit but before receipt");
    run_migrations_from(&pool, &migrations)
        .await
        .expect("V119 reruns safely without its migration receipt");
    assert_eq!(genesis_rows(&pool).await, final_genesis_snapshot);
    assert_eq!(sqlite_schema_snapshot(&pool).await, final_schema);
    let fk_errors: Vec<(String, i64, String, i64)> = sqlx::query_as("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("foreign key check");
    assert!(
        fk_errors.is_empty(),
        "V119 preserves FK integrity: {fk_errors:?}"
    );

    // A deletion guard alone cannot bypass the row-value fence. This guard
    // belongs to the surviving Project so it cannot authorize the teardown
    // below, which targets fk-project.
    sqlx::query(
        "INSERT INTO project_deletion_guard (project_id, created_at)
         VALUES ('fk-keep-project', ?)",
    )
    .bind(&now)
    .execute(&pool)
    .await
    .expect("guard fixture for semantic write denial");
    let denied_writes = [
        sqlx::query("UPDATE agent_commitment SET status = 'completed' WHERE id = 'fk-commitment'")
            .execute(&pool)
            .await,
        sqlx::query("UPDATE memory_item SET body = 'mutated' WHERE id = 'fk-memory'")
            .execute(&pool)
            .await,
        sqlx::query(
            "UPDATE product_genesis_session SET lifecycle = 'cancelled' WHERE id = 'fk-genesis'",
        )
        .execute(&pool)
        .await,
        sqlx::query("UPDATE product_genesis_session SET project_id = NULL WHERE id = 'fk-genesis'")
            .execute(&pool)
            .await,
        sqlx::query(
            "UPDATE product_genesis_session
             SET project_id = NULL, prompt_body = 'mixed semantic write'
             WHERE id = 'fk-genesis'",
        )
        .execute(&pool)
        .await,
        sqlx::query("UPDATE project_agent_binding SET state = 'paused' WHERE id = 'fk-binding-b'")
            .execute(&pool)
            .await,
        sqlx::query("UPDATE project_charter SET lifecycle = 'cancelled' WHERE id = 'fk-charter'")
            .execute(&pool)
            .await,
        sqlx::query(
            "UPDATE project_execution_baseline SET lifecycle = 'active' WHERE id = 'fk-baseline'",
        )
        .execute(&pool)
        .await,
        sqlx::query("UPDATE project_milestone SET lifecycle = 'active' WHERE id = 'fk-milestone'")
            .execute(&pool)
            .await,
        sqlx::query(
            "UPDATE project_task_governance SET runnable = 1 WHERE task_id = 'fk-task-keep'",
        )
        .execute(&pool)
        .await,
        sqlx::query(
            "UPDATE agent_commitment SET originating_task_id = NULL WHERE id = 'fk-commitment'",
        )
        .execute(&pool)
        .await,
        sqlx::query(
            "UPDATE agent_commitment SET originating_task_id = NULL, status = 'completed'
             WHERE id = 'fk-commitment'",
        )
        .execute(&pool)
        .await,
        sqlx::query(
            "UPDATE project_agent_binding
             SET replaced_by_binding_id = NULL, state = 'paused' WHERE id = 'fk-binding-a'",
        )
        .execute(&pool)
        .await,
        sqlx::query(
            "UPDATE project_charter_approval SET consumed_project_id = NULL
             WHERE id = 'fk-consumed-approval'",
        )
        .execute(&pool)
        .await,
        sqlx::query(
            "UPDATE project_charter_approval
             SET consumed_project_id = NULL, lifecycle = 'revoked'
             WHERE id = 'fk-consumed-approval'",
        )
        .execute(&pool)
        .await,
    ];
    for (index, result) in denied_writes.into_iter().enumerate() {
        assert!(result.is_err(), "retired or mixed update {index} must fail");
    }
    sqlx::query("DELETE FROM project_deletion_guard WHERE project_id = 'fk-keep-project'")
        .execute(&pool)
        .await
        .expect("remove test guard");

    db::ProjectRepo::delete(&db, "fk-project")
        .await
        .expect("productive Project teardown permits only exact FK cleanup");
    let genesis_after_delete: (
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT lifecycle, project_id, handoff_id, charter_id,
                charter_revision_id, charter_approval_id
         FROM product_genesis_session WHERE id = 'fk-genesis'",
    )
    .fetch_one(&pool)
    .await
    .expect("handed-off Genesis remains historical");
    assert_eq!(
        genesis_after_delete,
        ("handed_off".to_owned(), None, None, None, None, None),
        "legitimate parent deletion clears only the exact historical references"
    );
    let genesis_history: (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
    ) = sqlx::query_as(
        "SELECT account_id, main_chat_id, prompt_revision, prompt_body, maturity,
                initial_idea, source_message_ids_json,
                preferred_project_agent_identity_id, failure_reason, version
         FROM product_genesis_session WHERE id = 'fk-genesis'",
    )
    .fetch_one(&pool)
    .await
    .expect("non-FK Genesis history survives");
    assert_eq!(
        genesis_history,
        (
            "fk-owner".to_owned(),
            main_chat_id.clone(),
            "v7".to_owned(),
            "history".to_owned(),
            "mvp".to_owned(),
            "historical idea".to_owned(),
            r#"["fk-genesis-source"]"#.to_owned(),
            Some("fk-agent".to_owned()),
            None,
            7,
        ),
        "all non-FK Genesis fields remain unchanged"
    );
    let genesis_timestamps_and_charter_version: (String, String, i64) = sqlx::query_as(
        "SELECT created_at, updated_at, charter_version
         FROM product_genesis_session WHERE id = 'fk-genesis'",
    )
    .fetch_one(&pool)
    .await
    .expect("Genesis timestamps and Charter version survive");
    assert_eq!(
        genesis_timestamps_and_charter_version,
        (now.clone(), now.clone(), 4),
        "Genesis timestamps and Charter version remain unchanged"
    );
    let commitment: (String, String, Option<String>) = sqlx::query_as(
        "SELECT title, status, originating_task_id FROM agent_commitment WHERE id = 'fk-commitment'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical Commitment survives Task deletion");
    assert_eq!(
        commitment,
        ("historical commitment".to_owned(), "open".to_owned(), None)
    );
    let memory: (String, String, Option<String>, Option<String>) = sqlx::query_as(
        "SELECT title, body, task_id, execution_id FROM memory_item WHERE id = 'fk-memory'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical MemoryItem survives Task/Execution deletion");
    assert_eq!(
        memory,
        (
            "historical memory".to_owned(),
            "body preserved by teardown".to_owned(),
            None,
            None,
        )
    );
    let governance: (String, Option<String>, i64) = sqlx::query_as(
        "SELECT task_id, replacement_of_task_id, runnable
         FROM project_task_governance WHERE task_id = 'fk-task-keep'",
    )
    .fetch_one(&pool)
    .await
    .expect("historical governance survives target Task deletion");
    assert_eq!(governance, ("fk-task-keep".to_owned(), None, 0));

    let approval: (String, Option<String>, Option<String>, String) = sqlx::query_as(
        "SELECT lifecycle, consumed_project_id, consumed_at, updated_at
         FROM project_charter_approval WHERE id = 'fk-consumed-approval'",
    )
    .fetch_one(&pool)
    .await
    .expect("surviving Project OS approval remains historical");
    assert_eq!(
        approval,
        ("consumed".to_owned(), None, Some(now.clone()), now.clone(),),
        "only the consumed Project FK is cleared"
    );
    let mixed_approval_update = sqlx::query(
        "UPDATE project_charter_approval
         SET consumed_project_id = NULL, lifecycle = 'revoked'
         WHERE id = 'fk-consumed-approval'",
    )
    .execute(&pool)
    .await;
    assert!(
        mixed_approval_update.is_err(),
        "FK cleanup cannot authorize a Charter approval lifecycle change"
    );

    let binding: (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "SELECT state, identity_id, profile_id, replaced_by_binding_id, replacement_reason
             FROM project_agent_binding WHERE id = 'fk-binding-a'",
    )
    .fetch_one(&pool)
    .await
    .expect("older replacement binding survives");
    assert_eq!(
        binding,
        (
            "replaced".to_owned(),
            "fk-agent".to_owned(),
            Some("fk-profile".to_owned()),
            None,
            Some("historical replacement chain".to_owned()),
        )
    );
    let guard_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM project_deletion_guard")
        .fetch_one(&pool)
        .await
        .expect("Project teardown guard removed");
    assert_eq!(guard_count, 0);
    let semantic_write =
        sqlx::query("UPDATE project_agent_binding SET state = 'paused' WHERE id = 'fk-binding-a'")
            .execute(&pool)
            .await;
    assert!(
        semantic_write.is_err(),
        "self-FK cleanup cannot authorize a state change"
    );
}

#[tokio::test]
async fn v120_keeps_task_roles_separate_from_workspace_lease_classes() {
    struct LeaseCase {
        execution_role: &'static str,
        task_type: &'static str,
        task_role: &'static str,
        lease_role: &'static str,
        purpose: &'static str,
        capability: &'static str,
    }
    const CASES: [LeaseCase; 10] = [
        LeaseCase {
            execution_role: "implementer",
            task_type: "implementation",
            task_role: "implementer",
            lease_role: "worker",
            purpose: "implement",
            capability: "repository_write",
        },
        LeaseCase {
            execution_role: "planner",
            task_type: "planning",
            task_role: "planner",
            lease_role: "worker",
            purpose: "plan",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "reviewer",
            task_type: "review",
            task_role: "reviewer",
            lease_role: "reviewer",
            purpose: "review",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "validator",
            task_type: "validation",
            task_role: "validator",
            lease_role: "worker",
            purpose: "validate",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "investigator",
            task_type: "discovery",
            task_role: "investigator",
            lease_role: "worker",
            purpose: "investigate",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "interactive",
            task_type: "implementation",
            task_role: "implementer",
            lease_role: "worker",
            purpose: "general",
            capability: "repository_write",
        },
        LeaseCase {
            execution_role: "interactive",
            task_type: "planning",
            task_role: "planner",
            lease_role: "worker",
            purpose: "general",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "interactive",
            task_type: "review",
            task_role: "reviewer",
            lease_role: "worker",
            purpose: "general",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "interactive",
            task_type: "validation",
            task_role: "validator",
            lease_role: "worker",
            purpose: "general",
            capability: "repository_read",
        },
        LeaseCase {
            execution_role: "interactive",
            task_type: "discovery",
            task_role: "investigator",
            lease_role: "worker",
            purpose: "general",
            capability: "repository_read",
        },
    ];

    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("in-memory database");
    run_migrations(&pool)
        .await
        .expect("all migrations through V120");
    let db = db::SqliteDb::new(pool.clone());
    let base = chrono::Utc::now() + chrono::Duration::minutes(10);
    let base_text = base.to_rfc3339();
    let project_id = db::new_uuid_v4();
    let repo_id = db::new_uuid_v4();
    sqlx::query(
        "INSERT INTO project (id, name, settings, workflow_definition, created_at, updated_at)
         VALUES (?, 'V120 lease matrix', '{}', '{}', ?, ?)",
    )
    .bind(&project_id)
    .bind(&base_text)
    .bind(&base_text)
    .execute(&pool)
    .await
    .expect("Project fixture");
    sqlx::query(
        "INSERT INTO repo
         (id, project_id, name, remote_url, local_path, work_mode, default_branch,
          created_at, updated_at)
         VALUES (?, ?, 'lease-matrix', 'https://example.test/lease-matrix.git', NULL,
                 'direct_merge', 'main', ?, ?)",
    )
    .bind(&repo_id)
    .bind(&project_id)
    .bind(&base_text)
    .bind(&base_text)
    .execute(&pool)
    .await
    .expect("repository fixture");

    let active_actor = db::new_uuid_v4();
    let wrong_role_actor = db::new_uuid_v4();
    let stale_assignee_actor = db::new_uuid_v4();
    for (id, name) in [
        (&active_actor, "active role member"),
        (&wrong_role_actor, "wrong TaskRole member"),
        (&stale_assignee_actor, "legacy assignee only"),
    ] {
        AgentRepo::create(
            &db,
            CreateAgent {
                id: id.clone(),
                name: name.to_owned(),
                description: None,
                executor_type: "shell".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: None,
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: base_text.clone(),
                updated_at: base_text.clone(),
            },
        )
        .await
        .expect("global Agent fixture");
    }

    let read_digest = "sha256:6035ec533a0bdb74c461ea9ea2d7147a2e47ba7c8b54c8b732052ceec23e8234";
    let write_digest = "sha256:eeb061a14ab862e1a7b16989ef637293ba538f46122ff28b30313d330dbae4a8";
    let mut expected_lease_ids = BTreeSet::new();
    let mut membership_ids = Vec::new();

    for (index, case) in CASES.iter().enumerate() {
        let suffix = format!("{index:02}");
        let task_id = format!("v120-task-{suffix}");
        let task_role_id = format!("v120-role-{suffix}");
        let wrong_role_id = format!("v120-wrong-role-{suffix}");
        let membership_id = format!("v120-membership-{suffix}");
        let wrong_membership_id = format!("v120-wrong-membership-{suffix}");
        let execution_id = format!("v120-execution-{suffix}");
        let wrong_role_execution_id = format!("v120-wrong-role-execution-{suffix}");
        let stale_execution_id = format!("v120-stale-assignee-execution-{suffix}");
        let task_time = (base + chrono::Duration::seconds(index as i64)).to_rfc3339();
        sqlx::query(
            "INSERT INTO task
             (id, project_id, repo_id, title, task_type, status, assignee_type,
              assignee_id, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'in_progress', 'agent', ?, ?, ?)",
        )
        .bind(&task_id)
        .bind(&project_id)
        .bind(&repo_id)
        .bind(format!("V120 {} task", case.execution_role))
        .bind(case.task_type)
        .bind(&stale_assignee_actor)
        .bind(&task_time)
        .bind(&task_time)
        .execute(&pool)
        .await
        .expect("Task fixture with stale legacy assignee projection");
        sqlx::query(
            "INSERT INTO task_role (id, task_id, role, coordination_mode, policy_json,
                                    created_at, updated_at)
             VALUES (?, ?, ?, 'collaborative', '{}', ?, ?),
                    (?, ?, 'orchestrator', 'collaborative', '{}', ?, ?)",
        )
        .bind(&task_role_id)
        .bind(&task_id)
        .bind(case.task_role)
        .bind(&task_time)
        .bind(&task_time)
        .bind(&wrong_role_id)
        .bind(&task_id)
        .bind(&task_time)
        .bind(&task_time)
        .execute(&pool)
        .await
        .expect("canonical and wrong TaskRoles");
        sqlx::query(
            "INSERT INTO role_membership
             (id, task_role_id, actor_kind, actor_id, status, created_at, updated_at)
             VALUES (?, ?, 'agent', ?, 'active', ?, ?),
                    (?, ?, 'agent', ?, 'active', ?, ?)",
        )
        .bind(&membership_id)
        .bind(&task_role_id)
        .bind(&active_actor)
        .bind(&task_time)
        .bind(&task_time)
        .bind(&wrong_membership_id)
        .bind(&wrong_role_id)
        .bind(&wrong_role_actor)
        .bind(&task_time)
        .bind(&task_time)
        .execute(&pool)
        .await
        .expect("active exact and wrong TaskRole memberships");
        membership_ids.push((membership_id, task_time.clone()));

        for (id, actor_id) in [
            (&execution_id, &active_actor),
            (&wrong_role_execution_id, &wrong_role_actor),
            (&stale_execution_id, &stale_assignee_actor),
        ] {
            sqlx::query(
                "INSERT INTO execution
                 (id, task_id, agent_id, role, status, actor_kind, actor_id,
                  purpose, created_at, updated_at)
                 VALUES (?, ?, ?, ?, 'running', 'agent', ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(&task_id)
            .bind(actor_id)
            .bind(case.execution_role)
            .bind(actor_id)
            .bind(case.purpose)
            .bind(&task_time)
            .bind(&task_time)
            .execute(&pool)
            .await
            .expect("exact running Execution fixture");
        }
        let (invalid_review_role, invalid_review_purpose) = if case.execution_role == "reviewer" {
            (case.execution_role, "general")
        } else {
            (case.execution_role, "review")
        };
        let invalid_review_execution = sqlx::query(
            "INSERT INTO execution
             (id, task_id, agent_id, role, status, actor_kind, actor_id,
              purpose, created_at, updated_at)
             VALUES (?, ?, ?, ?, 'running', 'agent', ?, ?, ?, ?)",
        )
        .bind(format!("v120-invalid-review-contract-{suffix}"))
        .bind(&task_id)
        .bind(&active_actor)
        .bind(invalid_review_role)
        .bind(&active_actor)
        .bind(invalid_review_purpose)
        .bind(&task_time)
        .bind(&task_time)
        .execute(&pool)
        .await
        .expect_err("non-canonical Review Execution is fenced");
        assert!(
            invalid_review_execution
                .to_string()
                .contains("Review Execution must use role reviewer and purpose review"),
            "formal Review guard rejects {} + {} for {} Task: {invalid_review_execution}",
            invalid_review_role,
            invalid_review_purpose,
            case.task_type
        );
        let capability_digest = match case.capability {
            "repository_read" => read_digest,
            "repository_write" => write_digest,
            other => panic!("unexpected capability class {other}"),
        };
        let issued_at = (base - chrono::Duration::seconds(5)).to_rfc3339();
        let expires_at = (base + chrono::Duration::minutes(1)).to_rfc3339();
        let make_lease = |id: String,
                          execution_id: &str,
                          actor_id: &str,
                          lease_role: &str,
                          operation_key: String| CreateWorkspaceLease {
            id,
            project_id: project_id.clone(),
            task_id: task_id.clone(),
            work_unit_id: None,
            workspace_id: None,
            task_version: 1,
            execution_id: execution_id.to_owned(),
            operation_idempotency_key: operation_key,
            repository_binding_id: repo_id.clone(),
            base_ref: "main".to_owned(),
            role: lease_role.to_owned(),
            capabilities_json: serde_json::json!([case.capability]).to_string(),
            assigned_principal_type: "agent".to_owned(),
            assigned_principal_id: actor_id.to_owned(),
            capability_profile_revision: "forge.capability-profile/v1".to_owned(),
            capability_profile_digest: capability_digest.to_owned(),
            issuing_principal_type: "system".to_owned(),
            issuing_principal_id: "task-service-scheduler".to_owned(),
            issued_at: issued_at.clone(),
            expires_at: expires_at.clone(),
            created_at: issued_at.clone(),
            updated_at: issued_at.clone(),
        };

        let wrong_lease_role = if case.lease_role == "worker" {
            "reviewer"
        } else {
            "worker"
        };
        assert!(
            WorkspaceLeaseRepo::issue(
                &db,
                make_lease(
                    db::new_uuid_v4(),
                    &execution_id,
                    &active_actor,
                    wrong_lease_role,
                    execution_id.clone(),
                ),
            )
            .await
            .is_err(),
            "wrong lease class is denied for {} + {}",
            case.execution_role,
            case.task_type
        );
        assert!(
            WorkspaceLeaseRepo::issue(
                &db,
                make_lease(
                    db::new_uuid_v4(),
                    &wrong_role_execution_id,
                    &wrong_role_actor,
                    case.lease_role,
                    wrong_role_execution_id.clone(),
                ),
            )
            .await
            .is_err(),
            "wrong TaskRole member is denied for {} + {}",
            case.execution_role,
            case.task_type
        );
        assert!(
            WorkspaceLeaseRepo::issue(
                &db,
                make_lease(
                    db::new_uuid_v4(),
                    &stale_execution_id,
                    &stale_assignee_actor,
                    case.lease_role,
                    stale_execution_id.clone(),
                ),
            )
            .await
            .is_err(),
            "stale task.assignee_id does not authorize {} + {}",
            case.execution_role,
            case.task_type
        );

        let task_role: Option<String> =
            sqlx::query_scalar("SELECT role FROM task_role WHERE task_id = ? AND id = ?")
                .bind(&task_id)
                .bind(&task_role_id)
                .fetch_optional(&pool)
                .await
                .expect("exact TaskRole lookup");
        assert_eq!(task_role.as_deref(), Some(case.task_role));
        let authority: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution e
             JOIN role_membership rm ON rm.actor_id = e.actor_id
             JOIN task_role tr ON tr.id = rm.task_role_id
             WHERE e.id = ? AND e.task_id = ? AND e.actor_kind = 'agent'
               AND e.actor_id = ? AND e.agent_id = ? AND e.status = 'running'
               AND e.role = ? AND e.purpose = ?
               AND tr.id = ? AND tr.role = ? AND rm.status = 'active'",
        )
        .bind(&execution_id)
        .bind(&task_id)
        .bind(&active_actor)
        .bind(&active_actor)
        .bind(case.execution_role)
        .bind(case.purpose)
        .bind(&task_role_id)
        .bind(case.task_role)
        .fetch_one(&pool)
        .await
        .expect("exact TaskRole, Actor membership, and running Execution");
        assert_eq!(authority, 1, "exact authority fixture for case {index}");

        let lease = WorkspaceLeaseRepo::issue(
            &db,
            make_lease(
                db::new_uuid_v4(),
                &execution_id,
                &active_actor,
                case.lease_role,
                execution_id.clone(),
            ),
        )
        .await
        .unwrap_or_else(|error| {
            panic!(
                "valid {}/{} lease was denied: {error:?}",
                case.execution_role, case.task_type
            )
        });
        assert_eq!(lease.role, case.lease_role);
        assert_eq!(
            lease.capabilities_json,
            serde_json::json!([case.capability]).to_string()
        );
        expected_lease_ids.insert(lease.id);
    }

    let renew_at = (base + chrono::Duration::seconds(30)).to_rfc3339();
    let renewed_until = (base + chrono::Duration::minutes(15)).to_rfc3339();
    let renewed = WorkspaceLeaseRepo::renew_active(
        &db,
        &renew_at,
        &(base + chrono::Duration::minutes(2)).to_rfc3339(),
        &renewed_until,
        50,
    )
    .await
    .expect("all exact memberships renew while active");
    let renewed_ids: BTreeSet<String> = renewed.iter().map(|lease| lease.id.clone()).collect();
    assert_eq!(renewed_ids, expected_lease_ids);
    assert!(renewed.iter().all(|lease| lease.version == 2));

    for (membership_id, _) in membership_ids {
        let ended_at = (base + chrono::Duration::seconds(40)).to_rfc3339();
        sqlx::query(
            "UPDATE role_membership
             SET status = 'ended', ended_at = ?, updated_at = ?, version = version + 1
             WHERE id = ? AND status = 'active'",
        )
        .bind(&ended_at)
        .bind(&ended_at)
        .bind(membership_id)
        .execute(&pool)
        .await
        .expect("end exact TaskRole membership");
    }
    let after_membership_end = WorkspaceLeaseRepo::renew_active(
        &db,
        &(base + chrono::Duration::seconds(50)).to_rfc3339(),
        &(base + chrono::Duration::minutes(20)).to_rfc3339(),
        &(base + chrono::Duration::minutes(30)).to_rfc3339(),
        50,
    )
    .await
    .expect("ended memberships are skipped by renewal");
    assert!(after_membership_end.is_empty());
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
         VALUES ('pr11-charter', 'pr11-owner', 'pr11-handed-off-genesis',
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
        "INSERT INTO project_charter_approval
         (id, approval_type, charter_id, revision_id, content_digest, rendered_digest,
          expected_charter_version, approving_principal_type, approving_principal_id,
          authorization_basis, authorization_action, explicit_event,
          authorization_occurred_at, source_action, idempotency_key, created_at, updated_at)
         VALUES ('pr11-charter-approval', 'project_creation', 'pr11-charter',
                 'pr11-charter-r1', 'charter-digest', 'charter-render-digest', 1,
                 'user', 'pr11-owner', 'historical approval', 'project.create',
                 'approve historical charter', ?, 'fixture', 'pr11-charter-approval-key', ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical Charter approval");
    sqlx::query(
        "UPDATE product_genesis_session
         SET charter_id = 'pr11-charter', charter_revision_id = 'pr11-charter-r1',
             charter_approval_id = 'pr11-charter-approval', charter_version = 1
         WHERE id = 'pr11-handed-off-genesis'",
    )
    .execute(pool)
    .await
    .expect("Genesis Charter provenance");
    sqlx::query(
        "UPDATE project_agent_binding
         SET charter_id = 'pr11-charter', charter_revision_id = 'pr11-charter-r1'
         WHERE id = 'pr11-project-binding-global'",
    )
    .execute(pool)
    .await
    .expect("Project Agent binding Charter provenance");
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
