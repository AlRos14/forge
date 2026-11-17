//! V177: a Proposal may target a PR external action (plan node P3).

use crate::{create_sqlite_pool, run_migrations};

#[tokio::test]
async fn v177_proposal_guard_admits_only_an_existing_proposed_pr_external_action() {
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations(&pool).await.expect("migrations");
    let sql: String = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE name = 'proposal_actor_target_guard_insert'",
    )
    .fetch_one(&pool)
    .await
    .expect("guard exists exactly once");
    assert!(sql.contains("'pr_external_action'"));
    assert!(sql.contains("a.state = 'proposed'"));
    // Unknown target kinds stay rejected.
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&pool)
        .await
        .expect("pragma");
    let rejected = sqlx::query(
        "INSERT INTO proposal (id, task_id, proposer_actor_kind, proposer_actor_id, target_kind,
            target_id, action, reason, status, created_at)
         VALUES ('p1', 't1', 'human', 'u1', 'pr_external_action', 'missing', 'approve', 'r',
                 'open', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await;
    assert!(rejected.is_err(), "no such action or proposer");
    let unknown = sqlx::query(
        "INSERT INTO proposal (id, task_id, proposer_actor_kind, proposer_actor_id, target_kind,
            target_id, action, reason, status, created_at)
         VALUES ('p2', 't1', 'human', 'u1', 'nonsense', 'x', 'approve', 'r',
                 'open', '2026-01-01T00:00:00Z')",
    )
    .execute(&pool)
    .await;
    assert!(unknown.is_err());
}
