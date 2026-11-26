//! V184/V185 review ranges: migration preservation, guards, the transactional
//! ledger writers, fail-closed merge readiness and Project teardown.
//!
//! Fixtures drop admission triggers unrelated to the behaviour under test (as
//! the V178 tests do) so rows can be seeded directly; the guards being tested
//! are always left installed.

use super::{
    copy_migrations_through, ensure_task_role, insert_test_work_unit, multirepo_seed_repo,
    multirepo_seed_task, seed_agent, seed_project, seed_user, sqlite_db,
};
use crate::{
    create_sqlite_pool, new_uuid_v4, now_rfc3339, run_migrations, run_migrations_from,
    ActivateReviewCheckpoint, ActorKind, ActorRef, ApplyReviewCorrections, ArtifactKind,
    ArtifactStorageKind, CollaborationRepo, CreateArtifact, CreateDecision, CreateDomainEvent,
    CreateProposal, CreateRoleMembership, CreateWorkUnit, CreateWorkspace, DbError,
    DecisionOutcome, IngestRangeReport, PageRequest, ProjectRepo, ProposalTarget,
    ProposalTargetKind, ReviewAttributionKind, ReviewCheckpoint, ReviewCheckpointReason,
    ReviewCheckpointRepo, ReviewCheckpointState, ReviewCorrectionLink, ReviewFinding,
    ReviewFindingFilter, ReviewFindingLocation, ReviewFindingResolution, ReviewFindingSeverity,
    ReviewHumanDecision, ReviewLedgerRepo, ReviewLocationAttribution, ReviewPolicyDocument,
    ReviewPolicyRepo, ReviewPolicySource, ReviewRange, ReviewRangeContributor, ReviewRangeReport,
    ReviewRangeReportRepo, ReviewResolutionOutcome, ReviewReworkRound, ReviewRiskClass,
    ReviewSizeThreshold, ReviewTriggerMode, ReviewVerdict, RoleMembershipRepo,
    RoleMembershipStatus, SortBy, SortOrder, SqliteDb, TaskRepositoryRole, TaskRepositorySetRepo,
    TaskRepositoryWrite, UpsertQueuedReviewCheckpoint, WorkUnitReviewRange, WorkspaceRepo,
    WorkspaceStatus, WriteProjectReviewPolicy, WriteTaskReviewPolicy,
};
use sqlx::SqlitePool;

fn digest(character: char) -> String {
    character.to_string().repeat(64)
}

fn event(task_id: &str, user_id: &str, entity_id: &str) -> CreateDomainEvent {
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: "review_range.fixture".to_owned(),
        entity_type: "review_fixture".to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(user_id.to_owned()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: task_id.to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: None,
        payload_json: "{}".to_owned(),
        created_at: now_rfc3339(),
    }
}

fn policy_document() -> ReviewPolicyDocument {
    ReviewPolicyDocument {
        schema_version: 1,
        risk_class: ReviewRiskClass::R1,
        coverage_required: true,
        trigger_mode: ReviewTriggerMode::Auto,
        reviewer_role: "reviewer".to_owned(),
        required_reviewers: 1,
        require_human: false,
        size_threshold: ReviewSizeThreshold {
            changed_lines: 800,
            changed_files: 25,
        },
        coordinator_may_request: true,
        max_checkpoints_per_repo: 12,
    }
}

async fn execute<'q>(
    pool: &SqlitePool,
    sql: &'q str,
    binds: &[&'q str],
) -> Result<(), sqlx::Error> {
    let mut query = sqlx::query(sql);
    for bind in binds {
        query = query.bind(*bind);
    }
    query.execute(pool).await.map(|_| ())
}

async fn ok(pool: &SqlitePool, sql: &str, binds: &[&str]) {
    execute(pool, sql, binds)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"));
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
}

fn assert_rejected(result: Result<(), sqlx::Error>, expected: &str) {
    let error = result.expect_err("statement must be rejected");
    assert!(
        error.to_string().contains(expected),
        "expected `{expected}`, got `{error}`"
    );
}

/// Drops the table's triggers except `keep` and the coverage revision bumps.
async fn drop_triggers(pool: &SqlitePool, table: &str, keep: &[&str]) {
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'trigger' AND tbl_name = ?",
    )
    .bind(table)
    .fetch_all(pool)
    .await
    .expect("triggers list");
    for name in names {
        if keep.contains(&name.as_str()) || name.starts_with("review_coverage_bump") {
            continue;
        }
        ok(pool, &format!("DROP TRIGGER {name}"), &[]).await;
    }
}

struct World {
    db: SqliteDb,
    owner: String,
    project: String,
    repo: String,
    task: String,
    producer: String,
    reviewer: String,
    reviewer_role: String,
    reviewer_membership: String,
    producer_unit: String,
    workspace: String,
}

const EXECUTION_GUARDS: [&str; 4] = [
    "work_unit_review_independent_execution",
    "work_unit_range_review_independent_execution",
    "range_review_session_isolation_insert",
    "range_review_session_isolation_update",
];

async fn world() -> World {
    world_keeping(&EXECUTION_GUARDS).await
}

async fn world_keeping(execution_guards: &[&str]) -> World {
    let db = sqlite_db().await;
    let owner = seed_user(&db).await;
    let project = seed_project(&db, "Review ranges", Some(owner.clone())).await;
    let now = now_rfc3339();
    ok(
        db.pool(),
        "INSERT OR IGNORE INTO project_member (id, project_id, user_id, role, created_at, updated_at)
         VALUES (?, ?, ?, 'owner', ?, ?)",
        &[&new_uuid_v4(), &project, &owner, &now, &now],
    )
    .await;
    let repo = multirepo_seed_repo(&db, &project, "review-primary").await;
    let task = multirepo_seed_task(
        &db,
        &project,
        &repo,
        vec![TaskRepositoryWrite {
            repo_id: repo.clone(),
            role: TaskRepositoryRole::Primary,
        }],
    )
    .await;
    TaskRepositorySetRepo::get_current(&db, &task)
        .await
        .expect("repository set initializes")
        .expect("Task exists");
    let producer = seed_agent(&db, "range producer", "global", None).await;
    let reviewer = seed_agent(&db, "range reviewer", "global", None).await;
    let producer_unit =
        insert_test_work_unit(&db, &task, &producer, "implementer", true, Some(&producer)).await;
    ensure_task_role(&db, &task, "reviewer").await;
    let reviewer_role: String =
        sqlx::query_scalar("SELECT id FROM task_role WHERE task_id = ? AND role = 'reviewer'")
            .bind(&task)
            .fetch_one(db.pool())
            .await
            .expect("reviewer role");
    ok(
        db.pool(),
        "UPDATE task_role_capability_configuration SET configuration_json = ?
         WHERE task_role_id = ?",
        &[
            r#"{"schema_version":1,"display_label":null,"capabilities":["formal_review"],"rework":null}"#,
            &reviewer_role,
        ],
    )
    .await;
    let reviewer_membership = new_uuid_v4();
    RoleMembershipRepo::add(
        &db,
        CreateRoleMembership {
            id: reviewer_membership.clone(),
            task_role_id: reviewer_role.clone(),
            actor_kind: ActorKind::Agent,
            actor_id: reviewer.clone(),
            status: RoleMembershipStatus::Active,
            created_at: now.clone(),
            updated_at: now.clone(),
        },
    )
    .await
    .expect("reviewer membership");
    drop_triggers(db.pool(), "execution", execution_guards).await;
    for table in [
        "execution_task_role_authority",
        "harness_session",
        "work_unit_integration",
        "work_unit_repository_integration",
    ] {
        drop_triggers(db.pool(), table, &[]).await;
    }
    let workspace = WorkspaceRepo::create(
        &db,
        CreateWorkspace {
            id: new_uuid_v4(),
            task_id: task.clone(),
            repo_id: repo.clone(),
            worktree_path: "/tmp/forge-review-range-integration".to_owned(),
            branch: "integration/main".to_owned(),
            status: WorkspaceStatus::Ready,
            before_sha: Some("base-sha".to_owned()),
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await
    .expect("integration workspace")
    .id;
    World {
        db,
        owner,
        project,
        repo,
        task,
        producer,
        reviewer,
        reviewer_role,
        reviewer_membership,
        producer_unit,
        workspace,
    }
}

#[allow(clippy::too_many_arguments)]
async fn insert_execution(
    w: &World,
    id: &str,
    role: &str,
    actor_kind: &str,
    actor_id: &str,
    work_unit_id: &str,
    status: &str,
    session: Option<&str>,
) -> Result<(), sqlx::Error> {
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO execution (
            id, task_id, role, status, actor_kind, actor_id, agent_id,
            work_unit_id, work_unit_version, harness_session_id, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?, ?)",
    )
    .bind(id)
    .bind(&w.task)
    .bind(role)
    .bind(status)
    .bind(actor_kind)
    .bind(actor_id)
    .bind((actor_kind == "agent").then_some(actor_id))
    .bind(work_unit_id)
    .bind(session)
    .bind(&now)
    .bind(&now)
    .execute(w.db.pool())
    .await
    .map(|_| ())
}

async fn insert_session(w: &World, agent: &str, predecessor: Option<&str>) -> String {
    let id = new_uuid_v4();
    let now = now_rfc3339();
    sqlx::query(
        "INSERT INTO harness_session (
            id, agent_id, harness_kind, predecessor_session_id, created_at, updated_at
         ) VALUES (?, ?, 'shell', ?, ?, ?)",
    )
    .bind(&id)
    .bind(agent)
    .bind(predecessor)
    .bind(&now)
    .bind(&now)
    .execute(w.db.pool())
    .await
    .expect("harness session");
    id
}

/// One successful repository integration ending at `head`.
async fn integrate(w: &World, execution_id: &str, head: &str) -> String {
    let parent = new_uuid_v4();
    let child = new_uuid_v4();
    let now = now_rfc3339();
    ok(
        w.db.pool(),
        "INSERT INTO work_unit_integration (
            id, task_id, work_unit_id, execution_id, source_workspace_id, source_branch,
            source_sha, target_workspace_id, target_branch, target_before_sha,
            target_after_sha, operation_idempotency_key, outcome, started_at, finished_at,
            created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, 'work', ?, ?, 'integration/main', 'base-sha', ?, ?,
                   'success', ?, ?, ?, ?)",
        &[
            &parent,
            &w.task,
            &w.producer_unit,
            execution_id,
            &w.workspace,
            head,
            &w.workspace,
            head,
            &parent,
            &now,
            &now,
            &now,
            &now,
        ],
    )
    .await;
    ok(
        w.db.pool(),
        "INSERT INTO work_unit_repository_integration (
            id, work_unit_integration_id, task_id, work_unit_id, execution_id,
            repository_set_revision, repo_id, source_workspace_id, source_branch, source_sha,
            target_workspace_id, target_branch, target_before_sha, target_after_sha, outcome,
            started_at, finished_at, created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, 1, ?, ?, 'work', ?, ?, 'integration/main', 'base-sha', ?,
                   'success', ?, ?, ?, ?)",
        &[
            &child,
            &parent,
            &w.task,
            &w.producer_unit,
            execution_id,
            &w.repo,
            &w.workspace,
            head,
            &w.workspace,
            head,
            &now,
            &now,
            &now,
            &now,
        ],
    )
    .await;
    child
}

fn policy_write(w: &World, expected: Option<i64>, character: char) -> WriteTaskReviewPolicy {
    WriteTaskReviewPolicy {
        task_id: w.task.clone(),
        expected_revision: expected,
        policy: policy_document(),
        digest: digest(character),
        source: ReviewPolicySource::Owner,
        created_by_user_id: w.owner.clone(),
        gate_revisions: Vec::new(),
        initial_bases: Vec::new(),
        events: Vec::new(),
        created_at: now_rfc3339(),
    }
}

fn queued(w: &World, head: &str, reason: ReviewCheckpointReason) -> UpsertQueuedReviewCheckpoint {
    let now = now_rfc3339();
    UpsertQueuedReviewCheckpoint {
        checkpoint: ReviewCheckpoint {
            id: new_uuid_v4(),
            task_id: w.task.clone(),
            repo_id: w.repo.clone(),
            state: ReviewCheckpointState::Queued,
            reasons: vec![reason],
            attempt: 1,
            base_commit: None,
            head_commit: head.to_owned(),
            range_id: None,
            policy_revision: 1,
            blocked_code: None,
            blocked_detail_json: None,
            version: 1,
            created_at: now.clone(),
            updated_at: now,
        },
        request: None,
        supersede: Vec::new(),
        events: Vec::new(),
    }
}

async fn coverage_revision(w: &World) -> i64 {
    ReviewLedgerRepo::load_review_coverage_rows(&w.db, &w.task, &w.repo)
        .await
        .expect("coverage rows")
        .state_revision
}

struct Activated {
    checkpoint: ReviewCheckpoint,
    range: ReviewRange,
    review_unit: String,
    producer_execution: String,
    integration: String,
}

async fn activation(
    w: &World,
    checkpoint: &ReviewCheckpoint,
    integration: &str,
    contributors: &[(&str, ActorRef)],
) -> ActivateReviewCheckpoint {
    let now = now_rfc3339();
    let range_id = new_uuid_v4();
    let review_unit = new_uuid_v4();
    ActivateReviewCheckpoint {
        checkpoint_id: checkpoint.id.clone(),
        expected_version: checkpoint.version,
        expected_coverage_revision: coverage_revision(w).await,
        range: ReviewRange {
            id: range_id.clone(),
            task_id: w.task.clone(),
            repo_id: w.repo.clone(),
            repository_set_revision: 1,
            base_commit: "base-sha".to_owned(),
            head_commit: checkpoint.head_commit.clone(),
            base_tree: "base-tree".to_owned(),
            head_tree: "head-tree".to_owned(),
            range_digest: digest('1'),
            integration_workspace_id: w.workspace.clone(),
            head_integration_id: integration.to_owned(),
            created_at: now.clone(),
        },
        contributors: contributors
            .iter()
            .map(|(execution, actor)| ReviewRangeContributor {
                id: new_uuid_v4(),
                task_id: w.task.clone(),
                range_id: range_id.clone(),
                work_unit_id: Some(w.producer_unit.clone()),
                execution_id: Some((*execution).to_owned()),
                attribution_id: None,
                actor: actor.clone(),
                created_at: now.clone(),
            })
            .collect(),
        work_unit: CreateWorkUnit {
            id: review_unit.clone(),
            task_id: w.task.clone(),
            parent_work_unit_id: None,
            title: "Review range".to_owned(),
            scope: "Review the exact frozen range".to_owned(),
            execution_instructions: None,
            role: "reviewer".to_owned(),
            assigned_actor: None,
            requires_integration: false,
            provenance: None,
            artifact_input_ids: Vec::new(),
            repository_ids: vec![w.repo.clone()],
            review_approval_gate_id: None,
            review_approval_line_id: None,
            created_by: ActorRef::Human(w.owner.clone()),
            created_at: now.clone(),
        },
        review_range: WorkUnitReviewRange {
            review_work_unit_id: review_unit,
            task_id: w.task.clone(),
            checkpoint_id: checkpoint.id.clone(),
            range_id,
            repo_id: w.repo.clone(),
            input_findings: Vec::new(),
            input_findings_digest: digest('2'),
            created_at: now.clone(),
        },
        updated_at: now,
        events: Vec::new(),
    }
}

/// Policy, an Agent and a Human contribution, one integration and an active
/// (`pending`) checkpoint.
async fn activated(w: &World) -> Activated {
    ReviewPolicyRepo::write_task_review_policy(&w.db, policy_write(w, None, 'a'))
        .await
        .expect("policy");
    let producer_execution = new_uuid_v4();
    insert_execution(
        w,
        &producer_execution,
        "implementer",
        "agent",
        &w.producer,
        &w.producer_unit,
        "completed",
        None,
    )
    .await
    .expect("producer execution");
    let human_execution = new_uuid_v4();
    insert_execution(
        w,
        &human_execution,
        "implementer",
        "human",
        &w.owner,
        &w.producer_unit,
        "completed",
        None,
    )
    .await
    .expect("Human contributor execution");
    let integration = integrate(w, &producer_execution, "head-sha").await;
    let checkpoint = ReviewCheckpointRepo::upsert_queued_review_checkpoint(
        &w.db,
        queued(w, "head-sha", ReviewCheckpointReason::Baseline),
    )
    .await
    .expect("queued checkpoint");
    let input = activation(
        w,
        &checkpoint,
        &integration,
        &[
            (
                producer_execution.as_str(),
                ActorRef::Agent(w.producer.clone()),
            ),
            (human_execution.as_str(), ActorRef::Human(w.owner.clone())),
        ],
    )
    .await;
    let range = input.range.clone();
    let review_unit = input.work_unit.id.clone();
    let checkpoint = ReviewCheckpointRepo::activate_review_checkpoint(&w.db, input.clone())
        .await
        .expect("activation");
    let replay = ReviewCheckpointRepo::activate_review_checkpoint(&w.db, input)
        .await
        .expect("activation replay");
    assert_eq!(replay, checkpoint);
    Activated {
        checkpoint,
        range,
        review_unit,
        producer_execution,
        integration,
    }
}

struct Reported {
    report_artifact: String,
    p0: ReviewFinding,
    p1: ReviewFinding,
}

async fn reported(w: &World, active: &Activated) -> Reported {
    let execution = new_uuid_v4();
    insert_execution(
        w,
        &execution,
        "reviewer",
        "agent",
        &w.reviewer,
        &active.review_unit,
        "running",
        None,
    )
    .await
    .expect("reviewer execution");
    let now = now_rfc3339();
    ok(
        w.db.pool(),
        "INSERT INTO execution_task_role_authority (
            execution_id, task_role_id, task_role_version, configuration_json,
            capabilities_json, membership_id, actor_kind, actor_id, source, captured_at
         ) VALUES (?, ?, 1, '{}', '[\"formal_review\"]', ?, 'agent', ?, 'admission', ?)",
        &[
            &execution,
            &w.reviewer_role,
            &w.reviewer_membership,
            &w.reviewer,
            &now,
        ],
    )
    .await;
    let artifact_id = new_uuid_v4();
    let content = serde_json::json!({
        "kind": "review_report", "verdict": "request_changes", "summary": "two findings",
        "criteria": [], "questions": [], "evidence_considered": [], "findings": [],
        "resolutions": [],
        "subject": {
            "task_id": w.task, "review_execution_id": execution, "workspace_id": null,
            "base_commit_sha": null, "head_commit_sha": null,
            "workspace_snapshot_digest": null, "kind": "commit_range",
            "checkpoint_id": active.checkpoint.id,
            "range": {"id": active.range.id, "range_digest": active.range.range_digest}
        }
    })
    .to_string();
    let artifact = CreateArtifact {
        id: artifact_id.clone(),
        task_id: w.task.clone(),
        kind: ArtifactKind::ReviewReport,
        storage_kind: ArtifactStorageKind::Inline,
        content: Some(content),
        content_ref: None,
        metadata_json: "{}".to_owned(),
        digest: Some(digest('3')),
        producer_execution_id: execution.clone(),
        created_at: now.clone(),
    };
    // The generic writer cannot record a range report without its ledger.
    let generic = CollaborationRepo::create_execution_artifact_output(
        &w.db,
        artifact.clone(),
        event(&w.task, &w.owner, &artifact_id),
    )
    .await
    .expect_err("generic ReviewReport writer rejects a range review");
    assert!(matches!(generic, DbError::Check(_)));
    let finding = |ordinal: i64, severity: ReviewFindingSeverity| ReviewFinding {
        id: new_uuid_v4(),
        task_id: w.task.clone(),
        repo_id: w.repo.clone(),
        range_id: active.range.id.clone(),
        report_artifact_id: artifact_id.clone(),
        ordinal,
        label: format!("F-{ordinal}"),
        severity,
        title: "title".to_owned(),
        evidence: "evidence".to_owned(),
        invariant: None,
        anchor_commit: active.range.head_commit.clone(),
        digest: digest('4'),
        created_at: now.clone(),
    };
    let p0 = finding(0, ReviewFindingSeverity::P0);
    let p1 = finding(1, ReviewFindingSeverity::P1);
    let location = ReviewFindingLocation {
        finding_id: p1.id.clone(),
        task_id: w.task.clone(),
        ordinal: 0,
        path: "src/lib.rs".to_owned(),
        line_start: Some(1),
        line_end: Some(2),
        attribution: ReviewLocationAttribution {
            kind: ReviewAttributionKind::Unattributed,
            work_unit_id: None,
            execution_id: None,
            actor: None,
            commit_sha: None,
            integration_id: None,
            line_count: 0,
        },
    };
    let ingest = IngestRangeReport {
        report: ReviewRangeReport {
            report_artifact_id: artifact_id.clone(),
            task_id: w.task.clone(),
            repo_id: w.repo.clone(),
            range_id: active.range.id.clone(),
            checkpoint_id: active.checkpoint.id.clone(),
            review_work_unit_id: active.review_unit.clone(),
            execution_id: execution.clone(),
            actor: ActorRef::Agent(w.reviewer.clone()),
            verdict: ReviewVerdict::RequestChanges,
            report_digest: digest('3'),
            created_at: now,
        },
        findings: vec![
            (p0.clone(), Vec::new()),
            (p1.clone(), vec![location.clone()]),
        ],
        resolutions: Vec::new(),
        expected_checkpoint_version: active.checkpoint.version,
        next_checkpoint_state: ReviewCheckpointState::Reviewed,
        blocked_code: None,
        events: Vec::new(),
    };
    let stale = IngestRangeReport {
        expected_checkpoint_version: active.checkpoint.version + 1,
        ..ingest.clone()
    };
    let stale = ReviewRangeReportRepo::ingest_range_review_report(
        &w.db,
        artifact.clone(),
        Vec::new(),
        Vec::new(),
        event(&w.task, &w.owner, &artifact_id),
        stale,
    )
    .await
    .expect_err("stale checkpoint version");
    assert!(matches!(stale, DbError::VersionConflict));
    assert_eq!(
        count(w.db.pool(), "SELECT COUNT(*) FROM artifact").await,
        0,
        "a rejected report writes nothing"
    );
    let before = coverage_revision(w).await;
    let write = ReviewRangeReportRepo::ingest_range_review_report(
        &w.db,
        artifact.clone(),
        Vec::new(),
        Vec::new(),
        event(&w.task, &w.owner, &artifact_id),
        ingest.clone(),
    )
    .await
    .expect("range report ingests");
    assert!(write.event.is_some());
    assert!(coverage_revision(w).await > before);
    let replay = ReviewRangeReportRepo::ingest_range_review_report(
        &w.db,
        artifact,
        Vec::new(),
        Vec::new(),
        event(&w.task, &w.owner, &artifact_id),
        ingest,
    )
    .await
    .expect("range report replays");
    assert!(replay.event.is_none());
    assert_eq!(
        ReviewLedgerRepo::list_review_finding_locations(&w.db, &p1.id)
            .await
            .expect("locations"),
        vec![location]
    );
    Reported {
        report_artifact: artifact_id,
        p0,
        p1,
    }
}

fn waiver(w: &World, finding: &ReviewFinding) -> (ReviewHumanDecision, ReviewFindingResolution) {
    let now = now_rfc3339();
    let proposal_id = new_uuid_v4();
    let decision_id = new_uuid_v4();
    let actor = ActorRef::Human(w.owner.clone());
    (
        ReviewHumanDecision {
            proposal: CreateProposal {
                id: proposal_id.clone(),
                task_id: w.task.clone(),
                proposer: actor.clone(),
                target: ProposalTarget {
                    kind: ProposalTargetKind::ReviewFinding,
                    id: finding.id.clone(),
                },
                action: "waive_review_finding".to_owned(),
                reason: "accepted".to_owned(),
                target_version: None,
                target_digest: Some(finding.digest.clone()),
                required_policy_ref: None,
                required_policy_version: None,
                required_policy_digest: None,
                supersedes_proposal_id: None,
                artifact_ids: Vec::new(),
                created_at: now.clone(),
            },
            decision: CreateDecision {
                id: decision_id.clone(),
                task_id: w.task.clone(),
                proposal_id,
                proposal_version: 1,
                outcome: DecisionOutcome::Approve,
                rationale: "accepted".to_owned(),
                policy_ref: None,
                policy_version: None,
                policy_digest: None,
                actors: vec![actor.clone()],
                created_at: now.clone(),
            },
        },
        ReviewFindingResolution {
            id: new_uuid_v4(),
            finding_id: finding.id.clone(),
            task_id: w.task.clone(),
            repo_id: w.repo.clone(),
            outcome: ReviewResolutionOutcome::Waived,
            anchor_commit: finding.anchor_commit.clone(),
            report_artifact_id: None,
            decision_id: Some(decision_id),
            waiver_reference: Some("RISK-1".to_owned()),
            evidence: "accepted".to_owned(),
            actor,
            created_at: now,
        },
    )
}

#[tokio::test]
async fn review_range_migration_from_v183_preserves_gate_triage_and_coordination() {
    let migrations = tempfile::tempdir().expect("V183 migration directory");
    copy_migrations_through(183, migrations.path());
    let pool = create_sqlite_pool("sqlite::memory:").await.expect("pool");
    run_migrations_from(&pool, migrations.path())
        .await
        .expect("migrations through V183");
    let db = SqliteDb::new(pool.clone());
    let project = seed_project(&db, "V183 history", None).await;
    let repo = multirepo_seed_repo(&db, &project, "history-primary").await;
    let task = multirepo_seed_task(
        &db,
        &project,
        &repo,
        vec![TaskRepositoryWrite {
            repo_id: repo.clone(),
            role: TaskRepositoryRole::Primary,
        }],
    )
    .await;
    ensure_task_role(&db, &task, "implementer").await;
    let role: String = sqlx::query_scalar("SELECT id FROM task_role WHERE task_id = ?")
        .bind(&task)
        .fetch_one(&pool)
        .await
        .expect("role");
    drop_triggers(
        &pool,
        "execution",
        &["work_unit_review_independent_execution"],
    )
    .await;
    for table in [
        "artifact",
        "artifact_execution_producer",
        "task_review_triage",
        "task_coordination_request",
        "gate_policy_revision",
        "gate_evaluation",
    ] {
        drop_triggers(&pool, table, &[]).await;
    }
    let now = now_rfc3339();
    let d = digest('a');
    ok(
        &pool,
        "INSERT INTO execution (id, task_id, role, status, created_at, updated_at)
         VALUES ('execution-1', ?, 'implementer', 'completed', ?, ?)",
        &[&task, &now, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO artifact (id, task_id, kind, storage_kind, content, metadata_json, digest, created_at)
         VALUES ('artifact-1', ?, 'plan', 'inline', '{}', '{}', ?, ?)",
        &[&task, &d, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
         VALUES ('artifact-1', 'execution-1', ?)",
        &[&task],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO task_review_triage (
            review_report_artifact_id, task_id, report_digest, execution_id,
            authorization_version, disposition, rationale, instructions, decision_digest,
            review_work_unit_id, created_at
         ) VALUES ('artifact-1', ?, ?, 'execution-1', 3, 'human', 'why', 'how', ?, NULL, ?)",
        &[&task, &d, &d, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO task_coordination_request (
            id, task_id, authorization_version, mode, brief_json, objective_digest,
            request_digest, idempotency_key, state, created_at, updated_at
         ) VALUES ('request-1', ?, 1, 'proposal', '{}', ?, ?, 'key-1', 'requested', ?, ?)",
        &[&task, &d, &d, &now, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO gate (id, task_id, gate_kind, scope_kind, scope_id, created_at)
         VALUES ('gate-1', ?, 'delivery_acceptance', 'task', ?, ?)",
        &[&task, &task, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO gate_policy_revision (
            gate_id, revision, schema_version, policy_json, policy_digest, created_at
         ) VALUES ('gate-1', 1, 1, '{}', ?, ?)",
        &[&d, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO gate_evaluation (
            id, gate_id, task_id, policy_revision, outcome, input_digest, result_json, evaluated_at
         ) VALUES ('evaluation-1', 'gate-1', ?, 1, 'satisfied', ?, '{}', ?)",
        &[&task, &d, &now],
    )
    .await;
    ok(
        &pool,
        "INSERT INTO gate_evaluation_input (
            evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
            input_digest, producer_ref, subject_json, status
         ) VALUES ('evaluation-1', ?, 0, 'task_role_snapshot', ?, 1, ?, NULL, '{}', 'current')",
        &[&task, &role, &d],
    )
    .await;

    run_migrations(&pool).await.expect("V184 and V185 apply");

    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM _migration WHERE version IN (184, 185)"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM task_review_triage
             WHERE review_report_artifact_id = 'artifact-1' AND disposition = 'human'
               AND authorization_version = 3 AND instructions = 'how'",
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM gate_evaluation_input
             WHERE evaluation_id = 'evaluation-1' AND input_kind = 'task_role_snapshot'",
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM task_coordination_request WHERE id = 'request-1'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM task_review_policy_revision").await,
        0,
        "no policy is backfilled"
    );
    for name in [
        "idx_gate_evaluation_input_ref",
        "idx_review_checkpoint_active",
        "idx_task_review_policy_current",
        "gate_evaluation_input_same_task_insert",
        "gate_evaluation_input_immutable_update",
        "gate_evaluation_input_immutable_delete",
        "gate_evaluation_input_review_line_guard",
        "gate_evaluation_stale_input",
        "task_review_coordination_source_current",
        "task_review_triage_immutable",
        "task_review_triage_immutable_delete",
        "task_review_triage_current_request_guard",
        "review_coverage_push_update_guard",
    ] {
        let found: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE name = ?")
            .bind(name)
            .fetch_one(&pool)
            .await
            .expect("schema lookup");
        assert_eq!(found, 1, "missing {name}");
    }
    assert_rejected(
        execute(&pool, "UPDATE task_review_triage SET rationale = 'x'", &[]).await,
        "Review triage is immutable",
    );
    assert_rejected(
        execute(&pool, "UPDATE gate_evaluation_input SET status = 'x'", &[]).await,
        "Gate evaluation inputs are immutable",
    );
    // Views over rebuilt tables remain queryable.
    count(&pool, "SELECT COUNT(*) FROM gate_evaluation_currentness").await;
    count(
        &pool,
        "SELECT COUNT(*) FROM task_review_coordination_request_current",
    )
    .await;
    assert!(sqlx::query("PRAGMA foreign_key_check")
        .fetch_all(&pool)
        .await
        .expect("foreign key check")
        .is_empty());
}

#[tokio::test]
async fn review_range_policy_writes_are_versioned_authorized_and_copied() {
    let w = world().await;
    let first = ReviewPolicyRepo::write_task_review_policy(&w.db, policy_write(&w, None, 'a'))
        .await
        .expect("first revision");
    assert_eq!(first.revision, 1);
    assert_eq!(coverage_revision(&w).await, 1);
    let stale = ReviewPolicyRepo::write_task_review_policy(&w.db, policy_write(&w, None, 'b'))
        .await
        .expect_err("stale expected revision");
    assert!(matches!(stale, DbError::VersionConflict));
    let second = ReviewPolicyRepo::write_task_review_policy(&w.db, policy_write(&w, Some(1), 'b'))
        .await
        .expect("second revision");
    assert_eq!(second.revision, 2);
    assert_eq!(coverage_revision(&w).await, 2);
    let stranger = seed_user(&w.db).await;
    let mut unauthorized = policy_write(&w, Some(2), 'c');
    unauthorized.created_by_user_id = stranger;
    let unauthorized = ReviewPolicyRepo::write_task_review_policy(&w.db, unauthorized)
        .await
        .expect_err("non-admin cannot write a policy");
    assert!(matches!(unauthorized, DbError::ProjectAdminRequired));
    assert_rejected(
        execute(
            w.db.pool(),
            "UPDATE task_review_policy_revision SET digest = digest WHERE state = 'current'",
            &[],
        )
        .await,
        "immutable except supersession",
    );
    assert_rejected(
        execute(w.db.pool(), "DELETE FROM task_review_policy_revision", &[]).await,
        "preserved outside Project teardown",
    );
    assert_rejected(
        execute(
            w.db.pool(),
            "UPDATE task_review_coverage_state SET revision = revision + 2",
            &[],
        )
        .await,
        "advances exactly once",
    );

    ReviewPolicyRepo::write_project_review_policy(
        &w.db,
        WriteProjectReviewPolicy {
            project_id: w.project.clone(),
            expected_version: None,
            policy: Some((policy_document(), digest('d'))),
            updated_by_user_id: w.owner.clone(),
            updated_at: now_rfc3339(),
        },
    )
    .await
    .expect("Project default");
    let task = multirepo_seed_task(
        &w.db,
        &w.project,
        &w.repo,
        vec![TaskRepositoryWrite {
            repo_id: w.repo.clone(),
            role: TaskRepositoryRole::Primary,
        }],
    )
    .await;
    let copied = ReviewPolicyRepo::get_current_task_review_policy(&w.db, &task)
        .await
        .expect("copied policy lookup")
        .expect("default is copied at creation");
    assert_eq!(copied.source, ReviewPolicySource::ProjectDefault);
    assert_eq!(copied.digest, digest('d'));
}

#[tokio::test]
async fn review_range_checkpoints_are_single_active_immutable_and_exclusive() {
    let w = world().await;
    let active = activated(&w).await;
    assert_eq!(active.checkpoint.state, ReviewCheckpointState::Pending);

    let next = ReviewCheckpointRepo::upsert_queued_review_checkpoint(
        &w.db,
        queued(&w, "next-sha", ReviewCheckpointReason::Delta),
    )
    .await
    .expect("queued behind the active checkpoint");
    let merged = ReviewCheckpointRepo::upsert_queued_review_checkpoint(
        &w.db,
        queued(&w, "next-sha", ReviewCheckpointReason::Requested),
    )
    .await
    .expect("same head accumulates reasons");
    assert_eq!(merged.id, next.id);
    assert_eq!(
        merged.reasons,
        vec![
            ReviewCheckpointReason::Delta,
            ReviewCheckpointReason::Requested
        ]
    );
    let producer_execution = active.producer_execution.clone();
    let head = integrate(&w, &producer_execution, "next-sha").await;
    let mut second_range = activation(&w, &merged, &head, &[]).await;
    second_range.range.base_commit = "head-sha".to_owned();
    second_range.range.range_digest = digest('5');
    let conflict = ReviewCheckpointRepo::activate_review_checkpoint(&w.db, second_range)
        .await
        .expect_err("only one active checkpoint per repository");
    assert!(matches!(conflict, DbError::IdempotencyConflict));
    assert_eq!(
        count(w.db.pool(), "SELECT COUNT(*) FROM review_range").await,
        1,
        "a failed activation writes nothing"
    );

    for (sql, message) in [
        (
            "UPDATE review_range SET head_tree = 'x'",
            "review_range is immutable",
        ),
        (
            "UPDATE review_range_contributor SET actor_id = 'x'",
            "review_range_contributor is immutable",
        ),
        (
            "UPDATE work_unit_review_range SET input_findings_digest = input_findings_digest",
            "work_unit_review_range is immutable",
        ),
        (
            "DELETE FROM review_checkpoint",
            "preserved outside Project teardown",
        ),
        (
            "UPDATE review_checkpoint SET state = 'queued', version = version + 1
             WHERE state = 'pending'",
            "transition or identity is invalid",
        ),
    ] {
        assert_rejected(execute(w.db.pool(), sql, &[]).await, message);
    }

    drop_triggers(
        w.db.pool(),
        "work_unit_review_subject",
        &["work_unit_review_subject_excludes_range_insert"],
    )
    .await;
    assert_rejected(
        execute(
            w.db.pool(),
            "INSERT INTO work_unit_review_subject (
                review_work_unit_id, task_id, reviewed_work_unit_id, created_at
             ) VALUES (?, ?, ?, ?)",
            &[
                &active.review_unit,
                &w.task,
                &w.producer_unit,
                &now_rfc3339(),
            ],
        )
        .await,
        "either a result or a commit range",
    );
    // A range must end at an exact successful integration.
    assert_rejected(
        execute(
            w.db.pool(),
            "INSERT INTO review_range
             SELECT 'bad-range', task_id, repo_id, repository_set_revision, base_commit,
                    'unintegrated', base_tree, head_tree, range_digest,
                    integration_workspace_id, ?, created_at
             FROM review_range",
            &[&active.integration],
        )
        .await,
        "exact successful integration",
    );
}

#[tokio::test]
async fn review_range_independence_excludes_contributors_and_producer_context() {
    let w = world().await;
    let active = activated(&w).await;
    drop_triggers(
        w.db.pool(),
        "work_unit",
        &["work_unit_range_review_allocation"],
    )
    .await;
    assert_rejected(
        execute(
            w.db.pool(),
            "UPDATE work_unit SET assigned_actor_kind = 'human', assigned_actor_id = ?
             WHERE id = ?",
            &[&w.owner, &active.review_unit],
        )
        .await,
        "exact range review independence",
    );
    for (actor_kind, actor_id) in [("human", &w.owner), ("agent", &w.producer)] {
        assert_rejected(
            insert_execution(
                &w,
                &new_uuid_v4(),
                "reviewer",
                actor_kind,
                actor_id,
                &active.review_unit,
                "running",
                None,
            )
            .await,
            "exact range review independence",
        );
    }
    let produced = insert_session(&w, &w.producer, None).await;
    ok(
        w.db.pool(),
        "UPDATE execution SET harness_session_id = ? WHERE id = ?",
        &[&produced, &active.producer_execution],
    )
    .await;
    let forked = insert_session(&w, &w.producer, Some(&produced)).await;
    assert_rejected(
        insert_execution(
            &w,
            &new_uuid_v4(),
            "reviewer",
            "agent",
            &w.producer,
            &active.review_unit,
            "running",
            Some(&forked),
        )
        .await,
        "without the producer context",
    );
    let fresh = insert_session(&w, &w.producer, None).await;
    insert_execution(
        &w,
        &new_uuid_v4(),
        "reviewer",
        "agent",
        &w.producer,
        &active.review_unit,
        "running",
        Some(&fresh),
    )
    .await
    .expect("same Agent profile in an isolated session may review");
}

#[tokio::test]
async fn review_range_result_review_execution_guard_is_unchanged() {
    let w = world().await;
    let unit = insert_test_work_unit(&w.db, &w.task, &w.producer, "reviewer", false, None).await;
    assert_rejected(
        insert_execution(
            &w,
            &new_uuid_v4(),
            "reviewer",
            "agent",
            &w.reviewer,
            &unit,
            "running",
            None,
        )
        .await,
        "WorkUnit Execution violates exact review independence or readiness",
    );
}

#[tokio::test]
async fn review_range_reviewer_workspace_is_pinned_to_the_range_head() {
    let mut guards = EXECUTION_GUARDS.to_vec();
    guards.push("work_unit_range_review_admission_guard");
    let w = world_keeping(&guards).await;
    // Producer rows are not range WorkUnits, so the range guard ignores them.
    let active = activated(&w).await;
    assert_rejected(
        insert_execution(
            &w,
            &new_uuid_v4(),
            "reviewer",
            "agent",
            &w.reviewer,
            &active.review_unit,
            "running",
            None,
        )
        .await,
        "FORGE_WU_ADMISSION_REVIEW_GROUP_UNAVAILABLE",
    );
    let group = new_uuid_v4();
    let now = now_rfc3339();
    ok(
        w.db.pool(),
        "INSERT INTO workspace_group (id, task_id, work_unit_id, root_path, created_at)
         VALUES (?, ?, ?, '/tmp/forge-review-range-group', ?)",
        &[&group, &w.task, &active.review_unit, &now],
    )
    .await;
    drop_triggers(w.db.pool(), "workspace", &[]).await;
    for (sha, accepted) in [("base-sha", false), ("head-sha", true)] {
        let workspace = new_uuid_v4();
        ok(
            w.db.pool(),
            "INSERT INTO workspace (
                id, task_id, repo_id, worktree_path, branch, status, before_sha,
                created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 'ready', ?, ?, ?)",
            &[
                &workspace,
                &w.task,
                &w.repo,
                &format!("/tmp/forge-review-range-{sha}"),
                &format!("review/{sha}"),
                sha,
                &now,
                &now,
            ],
        )
        .await;
        let scoped = execute(
            w.db.pool(),
            "INSERT INTO workspace_scope (
                workspace_id, task_id, repo_id, scope_kind, work_unit_id, created_at,
                workspace_group_id
             ) VALUES (?, ?, ?, 'work_unit', ?, ?, ?)",
            &[
                &workspace,
                &w.task,
                &w.repo,
                &active.review_unit,
                &now,
                &group,
            ],
        )
        .await;
        if accepted {
            scoped.expect("reviewer scope at the range head");
        } else {
            assert_rejected(scoped, "outside its exact pinned group");
        }
    }
    insert_execution(
        &w,
        &new_uuid_v4(),
        "reviewer",
        "agent",
        &w.reviewer,
        &active.review_unit,
        "running",
        None,
    )
    .await
    .expect("prepared read-only group admits the reviewer");
}

#[tokio::test]
async fn review_range_report_ledger_is_atomic_and_p0_is_never_waivable() {
    let w = world().await;
    let active = activated(&w).await;
    let report = reported(&w, &active).await;
    let findings = ReviewLedgerRepo::list_review_findings(
        &w.db,
        &w.task,
        ReviewFindingFilter {
            repo_id: Some(w.repo.clone()),
            severity: Some(ReviewFindingSeverity::P1),
            finding_ids: None,
        },
        PageRequest {
            cursor: None,
            limit: 10,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .expect("findings");
    assert_eq!(findings.items, vec![report.p1.clone()]);

    let revision = coverage_revision(&w).await;
    let (decision, resolution) = waiver(&w, &report.p0);
    let refused =
        ReviewLedgerRepo::waive_review_finding(&w.db, revision, decision, resolution, Vec::new())
            .await
            .expect_err("P0 is never waivable");
    assert!(matches!(refused, DbError::Check(message) if message == "review.finding.not_waivable"));
    // The database refuses it independently of the writer.
    let (decision, _) = waiver(&w, &report.p0);
    assert!(CollaborationRepo::create_proposal(
        &w.db,
        decision.proposal,
        event(&w.task, &w.owner, &report.p0.id),
    )
    .await
    .is_err());

    let (decision, resolution) = waiver(&w, &report.p1);
    let stale = ReviewLedgerRepo::waive_review_finding(
        &w.db,
        revision - 1,
        decision.clone(),
        resolution.clone(),
        Vec::new(),
    )
    .await
    .expect_err("stale coverage revision");
    assert!(matches!(stale, DbError::VersionConflict));
    ReviewLedgerRepo::waive_review_finding(
        &w.db,
        revision,
        decision,
        resolution.clone(),
        Vec::new(),
    )
    .await
    .expect("P1 waiver");
    assert_eq!(coverage_revision(&w).await, revision + 1);
    let rows = ReviewLedgerRepo::load_review_coverage_rows(&w.db, &w.task, &w.repo)
        .await
        .expect("rows");
    assert_eq!(rows.resolutions, vec![resolution]);
    assert_eq!(rows.findings.len(), 2);
    assert_eq!(rows.reports.len(), 1);
    assert_eq!(rows.integrations.len(), 1, "single repository integration");
    assert_eq!(rows.checkpoints[0].state, ReviewCheckpointState::Reviewed);
    assert_rejected(
        execute(
            w.db.pool(),
            "UPDATE review_finding SET severity = 'P3'",
            &[],
        )
        .await,
        "review_finding is immutable",
    );
}

#[tokio::test]
async fn review_range_corrections_are_budgeted_idempotent_and_torn_down_with_project() {
    let w = world().await;
    let active = activated(&w).await;
    let report = reported(&w, &active).await;
    let now = now_rfc3339();
    let correction = new_uuid_v4();
    let mut unit = activation(&w, &active.checkpoint, &active.integration, &[])
        .await
        .work_unit;
    unit.id = correction.clone();
    unit.role = "implementer".to_owned();
    unit.requires_integration = true;
    let input = ApplyReviewCorrections {
        round: ReviewReworkRound {
            report_artifact_id: report.report_artifact.clone(),
            task_id: w.task.clone(),
            round: 1,
            request_id: None,
            created_by: ActorRef::Human(w.owner.clone()),
            idempotency_key: "round-1".to_owned(),
            created_at: now.clone(),
        },
        work_units: vec![unit],
        dependencies: Vec::new(),
        links: vec![ReviewCorrectionLink {
            correction_work_unit_id: correction,
            finding_id: report.p1.id.clone(),
            task_id: w.task.clone(),
            repo_id: w.repo.clone(),
            report_artifact_id: report.report_artifact.clone(),
            base_commit: "head-sha".to_owned(),
            created_at: now,
        }],
        max_rework_rounds: 0,
        events: Vec::new(),
    };
    assert!(
        ReviewLedgerRepo::apply_review_corrections(&w.db, input.clone())
            .await
            .is_err(),
        "exhausted round budget"
    );
    assert_eq!(
        count(w.db.pool(), "SELECT COUNT(*) FROM review_correction_link").await,
        0
    );
    let input = ApplyReviewCorrections {
        max_rework_rounds: 1,
        ..input
    };
    let first = ReviewLedgerRepo::apply_review_corrections(&w.db, input.clone())
        .await
        .expect("correction round");
    let replay = ReviewLedgerRepo::apply_review_corrections(&w.db, input)
        .await
        .expect("replay");
    assert_eq!(first, replay);
    assert_eq!(
        ReviewLedgerRepo::count_review_rework_rounds(&w.db, &w.task)
            .await
            .expect("count"),
        1
    );

    ProjectRepo::delete(&w.db, &w.project, Some(&w.owner))
        .await
        .expect("Project teardown removes review history");
    for table in [
        "task_review_policy_revision",
        "task_review_coverage_state",
        "review_range",
        "review_checkpoint",
        "review_range_report",
        "review_finding",
        "review_finding_location",
        "review_correction_link",
        "review_rework_round",
        "work_unit_review_range",
        "review_range_contributor",
    ] {
        assert_eq!(
            count(w.db.pool(), &format!("SELECT COUNT(*) FROM {table}")).await,
            0,
            "{table}"
        );
    }
}

#[tokio::test]
async fn review_range_merge_readiness_fails_closed_without_exact_coverage() {
    let w = world().await;
    activated(&w).await;
    for table in ["gate_policy_revision", "gate_evaluation"] {
        drop_triggers(w.db.pool(), table, &[]).await;
    }
    let now = now_rfc3339();
    ok(
        w.db.pool(),
        "INSERT INTO gate (id, task_id, gate_kind, scope_kind, scope_id, created_at)
         VALUES ('gate-merge', ?, 'merge_readiness', 'task', ?, ?)",
        &[&w.task, &w.task, &now],
    )
    .await;
    let v4 = serde_json::json!({
        "schema_version": 4,
        "review_coverage": {"review_policy_revision": 1, "review_policy_digest": digest('a')}
    })
    .to_string();
    for (revision, schema, policy) in [("1", "1", "{}"), ("2", "4", v4.as_str())] {
        ok(
            w.db.pool(),
            "INSERT INTO gate_policy_revision (
                gate_id, revision, schema_version, policy_json, policy_digest, created_at
             ) VALUES ('gate-merge', CAST(? AS INTEGER), CAST(? AS INTEGER), ?, ?, ?)",
            &[revision, schema, policy, &digest('b'), &now],
        )
        .await;
        ok(
            w.db.pool(),
            "INSERT INTO gate_evaluation (
                id, gate_id, task_id, policy_revision, outcome, input_digest, result_json,
                evaluated_at
             ) VALUES ('evaluation-' || ?, 'gate-merge', ?, CAST(? AS INTEGER), 'satisfied', ?,
                       '{}', ?)",
            &[revision, &w.task, revision, &digest('c'), &now],
        )
        .await;
    }
    assert!(
        coverage_missing(&w, "evaluation-1").await,
        "a v1 policy never covers"
    );
    assert!(
        coverage_missing(&w, "evaluation-2").await,
        "a changed repository needs its coverage input"
    );
    let revision = coverage_revision(&w).await.to_string();
    let subject = serde_json::json!({
        "review_policy_revision": 1, "review_policy_digest": digest('a'),
        "snapshot_digest": digest('e')
    })
    .to_string();
    let insert = "INSERT INTO gate_evaluation_input (
            evaluation_id, task_id, ordinal, input_kind, input_id, input_version,
            input_digest, producer_ref, subject_json, status
         ) VALUES ('evaluation-2', ?, 0, 'review_coverage', ?, CAST(? AS INTEGER), ?, NULL, ?,
                   'satisfied')";
    assert_rejected(
        execute(
            w.db.pool(),
            insert,
            &[&w.task, &w.repo, "999", &digest('e'), &subject],
        )
        .await,
        "Gate input is missing, stale",
    );
    ok(
        w.db.pool(),
        insert,
        &[&w.task, &w.repo, &revision, &digest('e'), &subject],
    )
    .await;
    assert!(!coverage_missing(&w, "evaluation-2").await);
    let stale = "SELECT COUNT(*) FROM gate_evaluation_stale_input
                 WHERE evaluation_id = 'evaluation-2'";
    assert_eq!(count(w.db.pool(), stale).await, 0);
    ReviewPolicyRepo::write_task_review_policy(&w.db, policy_write(&w, Some(1), 'f'))
        .await
        .expect("policy changes");
    assert_eq!(count(w.db.pool(), stale).await, 1);
    assert!(coverage_missing(&w, "evaluation-2").await);
}

async fn coverage_missing(w: &World, evaluation_id: &str) -> bool {
    let rows: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM gate_evaluation_review_coverage_missing WHERE evaluation_id = ?",
    )
    .bind(evaluation_id)
    .fetch_one(w.db.pool())
    .await
    .expect("coverage view");
    rows > 0
}
