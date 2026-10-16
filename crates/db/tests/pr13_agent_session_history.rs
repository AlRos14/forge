use db::{
    create_sqlite_pool, run_migrations_from, AgentRepo, AgentStatus, CreateAgentIdentity,
    CreateAgentProfile, SqliteDb,
};
use sqlx::SqlitePool;
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

async fn seed_v125_native_history(pool: &SqlitePool) {
    let db = SqliteDb::new(pool.clone());
    let now = "2026-10-13T09:30:00Z";
    AgentRepo::create_identity_with_profile(
        &db,
        CreateAgentIdentity {
            id: "pr13-history-agent".to_owned(),
            name: "Retired native Agent".to_owned(),
            description: None,
            max_concurrent_tasks: 1,
            heartbeat_interval_seconds: 30,
            max_missed_heartbeats: 3,
            status: AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: None,
            visibility: "global".to_owned(),
            account_permission_ceiling: "{}".to_owned(),
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
        CreateAgentProfile {
            id: "pr13-history-profile".to_owned(),
            identity_id: "pr13-history-agent".to_owned(),
            backend_kind: "native".to_owned(),
            executor_type: "embedded".to_owned(),
            provider: Some("historical-provider".to_owned()),
            model: Some("historical-model".to_owned()),
            reasoning_effort: None,
            permission_policy: None,
            prompt_template: None,
            capabilities_json: "{\"legacy\":true}".to_owned(),
            tool_policy_json: "{}".to_owned(),
            config_json: "{}".to_owned(),
            credential_ref: None,
            daemon_id: None,
            created_at: now.to_owned(),
            updated_at: now.to_owned(),
        },
    )
    .await
    .expect("historical Agent identity and profile");
    sqlx::query("DROP TRIGGER IF EXISTS pr11_retired_insert_agent_context_scope")
        .execute(pool)
        .await
        .expect("allow exact pre-cutover scope fixture");
    sqlx::query(
        "INSERT INTO agent_context_scope (
            id, identity_id, scope_type, scope_id, project_id, task_id,
            task_role, workspace_access, authority_json, created_at, updated_at
         ) VALUES (?, ?, 'account', 'former-account', NULL, NULL, NULL, 'deny', '{}', ?, ?)",
    )
    .bind("pr13-history-scope")
    .bind("pr13-history-agent")
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("historical scope fixture");

    sqlx::query(
        "INSERT INTO agent_session (
            id, identity_id, profile_id, context_scope_id, backend_kind,
            runtime_session_id, status, capabilities_json, connection_status,
            predecessor_session_id, replaced_by_session_id, last_activity_at,
            version, created_at, updated_at
         ) VALUES (
            'pr13-history-session', 'pr13-history-agent', 'pr13-history-profile',
            'pr13-history-scope', 'native', 'opaque-runtime-session', 'degraded',
            '{\"capability\":\"historical\"}', 'unavailable', NULL, NULL, ?, 4, ?, ?
         )",
    )
    .bind(now)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("legacy native session fixture");
    sqlx::query(
        "INSERT INTO protected_agent_session_state (
            session_id, snapshot_ciphertext, snapshot_nonce,
            checkpoint_ciphertext, checkpoint_nonce, checkpoint_turn_id,
            checkpoint_revision, checkpoint_fingerprint, key_revision,
            state_revision, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("pr13-history-session")
    .bind(vec![0, 255, 17, 34])
    .bind(vec![1, 2, 3])
    .bind(vec![255, 0, 85])
    .bind(vec![4, 5, 6])
    .bind("turn-original")
    .bind(9_i64)
    .bind("frozen-fingerprint")
    .bind(2_i64)
    .bind(11_i64)
    .bind(now)
    .execute(pool)
    .await
    .expect("encrypted session state fixture");
    sqlx::query(
        "INSERT INTO protected_interaction (
            id, session_id, interaction_kind, prompt_redacted,
            response_ciphertext, response_nonce, status, expires_at,
            version, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind("pr13-history-interaction")
    .bind("pr13-history-session")
    .bind("questionnaire")
    .bind("redacted prompt")
    .bind(vec![9, 8, 7, 0, 255])
    .bind(vec![6, 5, 4])
    .bind("answered")
    .bind(Option::<String>::None)
    .bind(3_i64)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await
    .expect("encrypted interaction fixture");
}

#[tokio::test]
async fn v126_renames_retired_agent_sessions_without_changing_protected_history() {
    let temp = tempfile::tempdir().expect("migration fixture directory");
    let v125 = temp.path().join("v125");
    let v126 = temp.path().join("v126");
    fs::create_dir_all(&v125).expect("V125 directory");
    fs::create_dir_all(&v126).expect("V126 directory");
    copy_migrations_through(125, &v125);
    copy_migrations_through(126, &v126);

    let pool = create_sqlite_pool("sqlite::memory:")
        .await
        .expect("memory database");
    run_migrations_from(&pool, &v125)
        .await
        .expect("V125 schema");
    seed_v125_native_history(&pool).await;
    run_migrations_from(&pool, &v126)
        .await
        .expect("V126 session history cutover");

    let old_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_session'",
    )
    .fetch_one(&pool)
    .await
    .expect("old table lookup");
    let history_table: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'agent_session_history'",
    )
    .fetch_one(&pool)
    .await
    .expect("history table lookup");
    assert_eq!(old_table, 0);
    assert_eq!(history_table, 1);

    let preserved: (String, String, String, String, String, i64) = sqlx::query_as(
        "SELECT identity_id, runtime_session_id, status, capabilities_json,
                connection_status, version
         FROM agent_session_history WHERE id = 'pr13-history-session'",
    )
    .fetch_one(&pool)
    .await
    .expect("exact session row remains");
    assert_eq!(preserved.0, "pr13-history-agent");
    assert_eq!(preserved.1, "opaque-runtime-session");
    assert_eq!(preserved.2, "degraded");
    assert_eq!(preserved.3, "{\"capability\":\"historical\"}");
    assert_eq!(preserved.4, "unavailable");
    assert_eq!(preserved.5, 4);

    let state: (Vec<u8>, Vec<u8>, Vec<u8>, String, i64, String) = sqlx::query_as(
        "SELECT snapshot_ciphertext, snapshot_nonce, checkpoint_ciphertext,
                checkpoint_turn_id, checkpoint_revision, checkpoint_fingerprint
         FROM protected_agent_session_state WHERE session_id = 'pr13-history-session'",
    )
    .fetch_one(&pool)
    .await
    .expect("protected session state remains");
    assert_eq!(state.0, [0, 255, 17, 34]);
    assert_eq!(state.1, [1, 2, 3]);
    assert_eq!(state.2, [255, 0, 85]);
    assert_eq!(state.3, "turn-original");
    assert_eq!(state.4, 9);
    assert_eq!(state.5, "frozen-fingerprint");

    let interaction: (Vec<u8>, Vec<u8>, String, String) = sqlx::query_as(
        "SELECT response_ciphertext, response_nonce, status, prompt_redacted
         FROM protected_interaction WHERE id = 'pr13-history-interaction'",
    )
    .fetch_one(&pool)
    .await
    .expect("protected interaction remains");
    assert_eq!(interaction.0, [9, 8, 7, 0, 255]);
    assert_eq!(interaction.1, [6, 5, 4]);
    assert_eq!(interaction.2, "answered");
    assert_eq!(interaction.3, "redacted prompt");

    for table in ["protected_agent_session_state", "protected_interaction"] {
        let references_history: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM pragma_foreign_key_list(?) WHERE \"table\" = 'agent_session_history'",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .expect("protected history FK lookup");
        assert_eq!(references_history, 1, "{table} must retain its history FK");
    }
    let receipt: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM _migration WHERE version = 126")
        .fetch_one(&pool)
        .await
        .expect("migration receipt lookup");
    assert_eq!(receipt, 1);
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("foreign key check")
        .is_empty());
}
