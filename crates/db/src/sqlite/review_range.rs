//! Review ranges, checkpoints, range reports and the finding ledger (V184/V185).
//!
//! Every writer runs in one `BEGIN IMMEDIATE` transaction, revalidates the rows
//! it depends on, and appends its domain events in that transaction. Git facts
//! are computed by callers; no transaction here waits for git.

use super::*;
use crate::{
    ActivateReviewCheckpoint, ActorRef, ApplyReviewCorrections, ApplyReviewTriage, Artifact,
    ArtifactKind, ArtifactStorageKind, CollaborationRepo, CreateArtifact, CreateDomainEvent,
    DecisionOutcome, DomainEventRepo, ExecutionArtifactOutputWrite, IngestRangeReport,
    ProjectReviewPolicy, ProposalTargetKind, ReviewCheckpoint, ReviewCheckpointReason,
    ReviewCheckpointRepo, ReviewCheckpointRequestRecord, ReviewCheckpointState,
    ReviewCommitAttribution, ReviewCorrectionLink, ReviewCoverageRows, ReviewFinding,
    ReviewFindingFilter, ReviewFindingLocation, ReviewFindingResolution, ReviewFindingSeverity,
    ReviewFindingTriage, ReviewHumanDecision, ReviewInputFinding, ReviewLedgerRepo,
    ReviewLocationAttribution, ReviewPlanCheckpoint, ReviewPolicyDocument, ReviewPolicyRepo,
    ReviewPolicyRevisionState, ReviewPolicySource, ReviewRange, ReviewRangeContributor,
    ReviewRangeReport, ReviewRangeReportRepo, ReviewResolutionOutcome, ReviewReworkRound,
    ReviewVerdict, TaskReviewBase, TaskReviewPolicyRevision, TransitionReviewCheckpoint,
    UpsertQueuedReviewCheckpoint, WorkUnitReviewRange, WriteProjectReviewPolicy,
    WriteTaskReviewPolicy,
};
use sqlx::SqliteConnection;

type Tx<'a> = Transaction<'a, Sqlite>;

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Unique collisions become idempotency conflicts; trigger and constraint
/// rejections keep their exact message.
fn write_error(error: sqlx::Error) -> DbError {
    if let Some(mapped) = super::project_plan_batch_write_error(&error) {
        return mapped;
    }
    if let sqlx::Error::Database(database_error) = &error {
        let message = database_error.message();
        if message.starts_with("FORGE_COORDINATION_") {
            return check_error(error);
        }
        if message.contains("UNIQUE constraint failed") {
            return DbError::IdempotencyConflict;
        }
        let constraint = database_error.code().is_some_and(|code| {
            matches!(
                code.as_ref(),
                "19" | "275" | "787" | "1299" | "1555" | "1811" | "2067"
            )
        });
        if constraint {
            return DbError::Check(message.to_owned());
        }
    }
    check_error(error)
}

fn invalid(message: &str) -> DbError {
    DbError::Check(message.to_owned())
}

fn ensure(condition: bool, message: &str) -> Result<()> {
    if condition {
        Ok(())
    } else {
        Err(invalid(message))
    }
}

fn from_json<T: serde::de::DeserializeOwned>(value: &str) -> Result<T> {
    serde_json::from_str(value)
        .map_err(|error| DbError::Check(format!("invalid persisted review JSON: {error}")))
}

fn to_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|error| DbError::Check(format!("review value cannot be serialized: {error}")))
}

fn actor_ref(kind: String, id: String) -> Result<ActorRef> {
    Ok(match parse_enum::<ActorKind>(kind)? {
        ActorKind::Human => ActorRef::Human(id),
        ActorKind::Agent => ActorRef::Agent(id),
    })
}

async fn append_events(db: &SqliteDb, tx: &mut Tx<'_>, events: &[CreateDomainEvent]) -> Result<()> {
    for event in events {
        DomainEventRepo::append_event_in_tx(db, tx, event).await?;
    }
    Ok(())
}

async fn task_project_id(tx: &mut Tx<'_>, task_id: &str) -> Result<String> {
    sqlx::query_scalar("SELECT project_id FROM task WHERE id = ? AND deleted_at IS NULL")
        .bind(task_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(DbError::NotFound)
}

async fn require_project_admin(tx: &mut Tx<'_>, project_id: &str, user_id: &str) -> Result<()> {
    let admin: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM project_member
         WHERE project_id = ? AND user_id = ? AND role IN ('owner', 'admin'))",
    )
    .bind(project_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    if admin {
        Ok(())
    } else {
        Err(DbError::ProjectAdminRequired)
    }
}

async fn require_review_waiver_authority(
    tx: &mut Tx<'_>,
    task_id: &str,
    user_id: &str,
) -> Result<()> {
    let authorized: bool = sqlx::query_scalar(
        "SELECT EXISTS (
             SELECT 1 FROM task t
             JOIN project_member pm ON pm.project_id = t.project_id
             WHERE t.id = ? AND pm.user_id = ? AND pm.role IN ('owner', 'admin')
         ) OR EXISTS (
             SELECT 1 FROM task_role tr
             JOIN role_membership rm ON rm.task_role_id = tr.id
             WHERE tr.task_id = ? AND rm.actor_kind = 'human' AND rm.actor_id = ?
               AND rm.status = 'active'
         )",
    )
    .bind(task_id)
    .bind(user_id)
    .bind(task_id)
    .bind(user_id)
    .fetch_one(&mut **tx)
    .await?;
    ensure(
        authorized,
        "review waiver requires an active Task member or Project owner/admin",
    )
}

async fn coverage_revision(
    conn: &mut SqliteConnection,
    task_id: &str,
    repo_id: &str,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT revision FROM task_review_coverage_state WHERE task_id = ? AND repo_id = ?",
    )
    .bind(task_id)
    .bind(repo_id)
    .fetch_optional(&mut *conn)
    .await?
    .unwrap_or(0))
}

// ---------------------------------------------------------------------------
// Row mappers
// ---------------------------------------------------------------------------

fn map_policy_revision(row: SqliteRow) -> Result<TaskReviewPolicyRevision> {
    Ok(TaskReviewPolicyRevision {
        task_id: row.try_get("task_id")?,
        revision: row.try_get("revision")?,
        state: parse_enum::<ReviewPolicyRevisionState>(row.try_get("state")?)?,
        policy: from_json(&row.try_get::<String, _>("policy_json")?)?,
        digest: row.try_get("digest")?,
        source: parse_enum::<ReviewPolicySource>(row.try_get("source")?)?,
        created_by_user_id: row.try_get("created_by_user_id")?,
        created_at: row.try_get("created_at")?,
        superseded_at: row.try_get("superseded_at")?,
    })
}

fn map_project_policy(row: SqliteRow) -> Result<ProjectReviewPolicy> {
    let policy = row
        .try_get::<Option<String>, _>("policy_json")?
        .map(|value| from_json::<ReviewPolicyDocument>(&value))
        .transpose()?;
    Ok(ProjectReviewPolicy {
        project_id: row.try_get("project_id")?,
        version: row.try_get("version")?,
        policy,
        digest: row.try_get("digest")?,
        updated_by_user_id: row.try_get("updated_by_user_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_base(row: SqliteRow) -> Result<TaskReviewBase> {
    Ok(TaskReviewBase {
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        revision: row.try_get("revision")?,
        base_commit: row.try_get("base_commit")?,
        base_tree: row.try_get("base_tree")?,
        source: parse_enum(row.try_get("source")?)?,
        decision_id: row.try_get("decision_id")?,
        pinned_at: row.try_get("pinned_at")?,
    })
}

fn map_attribution(row: SqliteRow) -> Result<ReviewCommitAttribution> {
    Ok(ReviewCommitAttribution {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        commit_sha: row.try_get("commit_sha")?,
        actor: actor_ref(row.try_get("actor_kind")?, row.try_get("actor_id")?)?,
        decision_id: row.try_get("decision_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_range(row: SqliteRow) -> Result<ReviewRange> {
    Ok(ReviewRange {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        repository_set_revision: row.try_get("repository_set_revision")?,
        base_commit: row.try_get("base_commit")?,
        head_commit: row.try_get("head_commit")?,
        base_tree: row.try_get("base_tree")?,
        head_tree: row.try_get("head_tree")?,
        range_digest: row.try_get("range_digest")?,
        integration_workspace_id: row.try_get("integration_workspace_id")?,
        head_integration_id: row.try_get("head_integration_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_checkpoint(row: SqliteRow) -> Result<ReviewCheckpoint> {
    Ok(ReviewCheckpoint {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        state: parse_enum(row.try_get("state")?)?,
        reasons: from_json(&row.try_get::<String, _>("reasons_json")?)?,
        attempt: row.try_get("attempt")?,
        base_commit: row.try_get("base_commit")?,
        head_commit: row.try_get("head_commit")?,
        range_id: row.try_get("range_id")?,
        policy_revision: row.try_get("policy_revision")?,
        blocked_code: row.try_get("blocked_code")?,
        blocked_detail_json: row.try_get("blocked_detail_json")?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_contributor(row: SqliteRow) -> Result<ReviewRangeContributor> {
    Ok(ReviewRangeContributor {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        range_id: row.try_get("range_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        execution_id: row.try_get("execution_id")?,
        attribution_id: row.try_get("attribution_id")?,
        actor: actor_ref(row.try_get("actor_kind")?, row.try_get("actor_id")?)?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_work_unit_review_range(row: SqliteRow) -> Result<WorkUnitReviewRange> {
    Ok(WorkUnitReviewRange {
        review_work_unit_id: row.try_get("review_work_unit_id")?,
        task_id: row.try_get("task_id")?,
        checkpoint_id: row.try_get("checkpoint_id")?,
        range_id: row.try_get("range_id")?,
        repo_id: row.try_get("repo_id")?,
        input_findings: from_json::<Vec<ReviewInputFinding>>(
            &row.try_get::<String, _>("input_findings_json")?,
        )?,
        input_findings_digest: row.try_get("input_findings_digest")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_report(row: SqliteRow) -> Result<ReviewRangeReport> {
    Ok(ReviewRangeReport {
        report_artifact_id: row.try_get("report_artifact_id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        range_id: row.try_get("range_id")?,
        checkpoint_id: row.try_get("checkpoint_id")?,
        review_work_unit_id: row.try_get("review_work_unit_id")?,
        execution_id: row.try_get("execution_id")?,
        actor: actor_ref(row.try_get("actor_kind")?, row.try_get("actor_id")?)?,
        verdict: parse_enum(row.try_get("verdict")?)?,
        report_digest: row.try_get("report_digest")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_finding(row: SqliteRow) -> Result<ReviewFinding> {
    Ok(ReviewFinding {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        range_id: row.try_get("range_id")?,
        report_artifact_id: row.try_get("report_artifact_id")?,
        ordinal: row.try_get("ordinal")?,
        label: row.try_get("label")?,
        severity: parse_enum(row.try_get("severity")?)?,
        title: row.try_get("title")?,
        evidence: row.try_get("evidence")?,
        invariant: row.try_get("invariant_text")?,
        anchor_commit: row.try_get("anchor_commit")?,
        digest: row.try_get("digest")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_location(row: SqliteRow) -> Result<ReviewFindingLocation> {
    let actor = match (
        row.try_get::<Option<String>, _>("attributed_actor_kind")?,
        row.try_get::<Option<String>, _>("attributed_actor_id")?,
    ) {
        (Some(kind), Some(id)) => Some(actor_ref(kind, id)?),
        _ => None,
    };
    Ok(ReviewFindingLocation {
        finding_id: row.try_get("finding_id")?,
        task_id: row.try_get("task_id")?,
        ordinal: row.try_get("ordinal")?,
        path: row.try_get("path")?,
        line_start: row.try_get("line_start")?,
        line_end: row.try_get("line_end")?,
        attribution: ReviewLocationAttribution {
            kind: parse_enum(row.try_get("attribution_kind")?)?,
            work_unit_id: row.try_get("attributed_work_unit_id")?,
            execution_id: row.try_get("attributed_execution_id")?,
            actor,
            commit_sha: row.try_get("attributed_commit_sha")?,
            integration_id: row.try_get("attributed_integration_id")?,
            line_count: row.try_get("attributed_line_count")?,
        },
    })
}

fn map_resolution(row: SqliteRow) -> Result<ReviewFindingResolution> {
    Ok(ReviewFindingResolution {
        id: row.try_get("id")?,
        finding_id: row.try_get("finding_id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        outcome: parse_enum(row.try_get("outcome")?)?,
        anchor_commit: row.try_get("anchor_commit")?,
        report_artifact_id: row.try_get("report_artifact_id")?,
        decision_id: row.try_get("decision_id")?,
        waiver_reference: row.try_get("waiver_reference")?,
        evidence: row.try_get("evidence")?,
        actor: actor_ref(row.try_get("actor_kind")?, row.try_get("actor_id")?)?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_triage(row: SqliteRow) -> Result<ReviewFindingTriage> {
    Ok(ReviewFindingTriage {
        finding_id: row.try_get("finding_id")?,
        request_id: row.try_get("request_id")?,
        task_id: row.try_get("task_id")?,
        report_artifact_id: row.try_get("report_artifact_id")?,
        execution_id: row.try_get("execution_id")?,
        authorization_version: row.try_get("authorization_version")?,
        disposition: parse_enum(row.try_get("disposition")?)?,
        rationale: row.try_get("rationale")?,
        instructions: row.try_get("instructions")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_link(row: SqliteRow) -> Result<ReviewCorrectionLink> {
    Ok(ReviewCorrectionLink {
        correction_work_unit_id: row.try_get("correction_work_unit_id")?,
        finding_id: row.try_get("finding_id")?,
        task_id: row.try_get("task_id")?,
        repo_id: row.try_get("repo_id")?,
        report_artifact_id: row.try_get("report_artifact_id")?,
        base_commit: row.try_get("base_commit")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_round(row: SqliteRow) -> Result<ReviewReworkRound> {
    Ok(ReviewReworkRound {
        report_artifact_id: row.try_get("report_artifact_id")?,
        task_id: row.try_get("task_id")?,
        round: row.try_get("round")?,
        request_id: row.try_get("request_id")?,
        created_by: actor_ref(
            row.try_get("created_by_kind")?,
            row.try_get("created_by_id")?,
        )?,
        idempotency_key: row.try_get("idempotency_key")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_plan_checkpoint(row: SqliteRow) -> Result<ReviewPlanCheckpoint> {
    Ok(ReviewPlanCheckpoint {
        task_id: row.try_get("task_id")?,
        plan_artifact_id: row.try_get("plan_artifact_id")?,
        plan_key: row.try_get("plan_key")?,
        work_unit_ids: from_json(&row.try_get::<String, _>("work_unit_ids_json")?)?,
        repository_ids: from_json(&row.try_get::<String, _>("repository_ids_json")?)?,
        reason: row.try_get("reason")?,
        active: row.try_get::<String, _>("state")? == "active",
        created_at: row.try_get("created_at")?,
    })
}

// ---------------------------------------------------------------------------
// Reads shared by writers
// ---------------------------------------------------------------------------

async fn current_policy(
    conn: &mut SqliteConnection,
    task_id: &str,
) -> Result<Option<TaskReviewPolicyRevision>> {
    sqlx::query("SELECT * FROM task_review_policy_revision WHERE task_id = ? AND state = 'current'")
        .bind(task_id)
        .fetch_optional(&mut *conn)
        .await?
        .map(map_policy_revision)
        .transpose()
}

async fn checkpoint_by_id(
    conn: &mut SqliteConnection,
    id: &str,
) -> Result<Option<ReviewCheckpoint>> {
    sqlx::query("SELECT * FROM review_checkpoint WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .map(map_checkpoint)
        .transpose()
}

async fn review_range_of_work_unit(
    conn: &mut SqliteConnection,
    work_unit_id: &str,
) -> Result<Option<WorkUnitReviewRange>> {
    sqlx::query("SELECT * FROM work_unit_review_range WHERE review_work_unit_id = ?")
        .bind(work_unit_id)
        .fetch_optional(&mut *conn)
        .await?
        .map(map_work_unit_review_range)
        .transpose()
}

async fn finding_by_id(conn: &mut SqliteConnection, id: &str) -> Result<Option<ReviewFinding>> {
    sqlx::query("SELECT * FROM review_finding WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?
        .map(map_finding)
        .transpose()
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

async fn insert_policy_revision(tx: &mut Tx<'_>, record: &TaskReviewPolicyRevision) -> Result<()> {
    sqlx::query(
        "INSERT INTO task_review_policy_revision (
             task_id, revision, state, policy_json, digest, risk_class,
             coverage_required, source, created_by_user_id, created_at, superseded_at
         ) VALUES (?, ?, 'current', ?, ?, ?, ?, ?, ?, ?, NULL)",
    )
    .bind(&record.task_id)
    .bind(record.revision)
    .bind(to_json(&record.policy)?)
    .bind(&record.digest)
    .bind(record.policy.risk_class.to_string())
    .bind(i64::from(record.policy.coverage_required))
    .bind(record.source.to_string())
    .bind(&record.created_by_user_id)
    .bind(&record.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

/// Whether the connected schema includes V184. Shared writers stay usable by
/// fixtures that exercise older schemas.
pub(super) async fn review_schema_present(tx: &mut Tx<'_>) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master
         WHERE type = 'table' AND name = 'project_review_policy')",
    )
    .fetch_one(&mut **tx)
    .await?)
}

/// Copies the Project default into a Task created in this transaction. There is
/// no later inheritance: the Task revision is the only authority afterwards.
pub(super) async fn copy_project_default_policy_in_tx(
    db: &SqliteDb,
    tx: &mut Tx<'_>,
    task_id: &str,
    project_id: &str,
    created_at: &str,
) -> Result<()> {
    if !review_schema_present(tx).await? {
        return Ok(());
    }
    let Some(default) = sqlx::query(
        "SELECT * FROM project_review_policy WHERE project_id = ? AND policy_json IS NOT NULL",
    )
    .bind(project_id)
    .fetch_optional(&mut **tx)
    .await?
    .map(map_project_policy)
    .transpose()?
    else {
        return Ok(());
    };
    let (Some(policy), Some(digest)) = (default.policy, default.digest) else {
        return Err(invalid("Project review policy default is incomplete"));
    };
    let record = TaskReviewPolicyRevision {
        task_id: task_id.to_owned(),
        revision: 1,
        state: ReviewPolicyRevisionState::Current,
        policy,
        digest,
        source: ReviewPolicySource::ProjectDefault,
        created_by_user_id: default.updated_by_user_id,
        created_at: created_at.to_owned(),
        superseded_at: None,
    };
    insert_policy_revision(tx, &record).await?;
    let event = CreateDomainEvent {
        id: crate::new_uuid_v4(),
        event_type: "task.review_policy_updated".to_owned(),
        entity_type: "task".to_owned(),
        entity_id: task_id.to_owned(),
        actor_type: "human".to_owned(),
        actor_id: Some(record.created_by_user_id.clone()),
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: task_id.to_owned(),
        causation_id: None,
        causation_depth: 0,
        dedupe_key: Some(format!("task-review-policy:{task_id}:1")),
        payload_json: serde_json::json!({
            "task_id": task_id,
            "revision": 1,
            "risk_class": record.policy.risk_class.to_string(),
            "coverage_required": record.policy.coverage_required,
        })
        .to_string(),
        created_at: created_at.to_owned(),
    };
    DomainEventRepo::append_event_in_tx(db, tx, &event).await?;
    Ok(())
}

#[async_trait]
impl ReviewPolicyRepo for SqliteDb {
    async fn get_current_task_review_policy(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskReviewPolicyRevision>> {
        let mut conn = self.pool.acquire().await?;
        current_policy(&mut conn, task_id).await
    }

    async fn write_task_review_policy(
        &self,
        input: WriteTaskReviewPolicy,
    ) -> Result<TaskReviewPolicyRevision> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let project_id = task_project_id(&mut tx, &input.task_id).await?;
        if input.source == ReviewPolicySource::Owner {
            require_project_admin(&mut tx, &project_id, &input.created_by_user_id).await?;
        }
        let current = current_policy(&mut tx, &input.task_id).await?;
        if current.as_ref().map(|policy| policy.revision) != input.expected_revision {
            let replay = current.filter(|policy| {
                policy.revision == input.expected_revision.unwrap_or(0) + 1
                    && policy.digest == input.digest
                    && policy.policy == input.policy
                    && policy.source == input.source
                    && policy.created_by_user_id == input.created_by_user_id
            });
            return replay.ok_or(DbError::VersionConflict);
        }
        // Coverage state is created for the Task's current repository set.
        super::task_repository::ensure_initial_current_revision_in_tx(&mut tx, &input.task_id)
            .await?;
        if let Some(current) = current.as_ref() {
            let superseded = sqlx::query(
                "UPDATE task_review_policy_revision SET state = 'superseded', superseded_at = ?
                 WHERE task_id = ? AND revision = ? AND state = 'current'",
            )
            .bind(&input.created_at)
            .bind(&input.task_id)
            .bind(current.revision)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            if superseded.rows_affected() != 1 {
                return Err(DbError::VersionConflict);
            }
        }
        let revision: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(revision), 0) + 1 FROM task_review_policy_revision
             WHERE task_id = ?",
        )
        .bind(&input.task_id)
        .fetch_one(&mut *tx)
        .await?;
        let record = TaskReviewPolicyRevision {
            task_id: input.task_id.clone(),
            revision,
            state: ReviewPolicyRevisionState::Current,
            policy: input.policy,
            digest: input.digest,
            source: input.source,
            created_by_user_id: input.created_by_user_id,
            created_at: input.created_at,
            superseded_at: None,
        };
        insert_policy_revision(&mut tx, &record).await?;
        for gate in &input.gate_revisions {
            ensure(
                gate.revision.gate_id == gate.gate_id
                    && gate.revision.expected_active_revision == gate.expected_active_revision,
                "Gate revision identity does not match its review policy write",
            )?;
            let owned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM gate
                 WHERE id = ? AND task_id = ? AND gate_kind = 'merge_readiness')",
            )
            .bind(&gate.gate_id)
            .bind(&record.task_id)
            .fetch_one(&mut *tx)
            .await?;
            ensure(
                owned,
                "review policy may revise only this Task's merge_readiness Gates",
            )?;
            super::gate_lifecycle::insert_gate_policy_revision_in_tx(self, &mut tx, &gate.revision)
                .await?;
        }
        for base in &input.initial_bases {
            ensure(
                base.task_id == record.task_id && base.revision == 1 && base.decision_id.is_none(),
                "initial review base must be this Task's revision 1",
            )?;
            let pinned: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM task_review_base WHERE task_id = ? AND repo_id = ?)",
            )
            .bind(&base.task_id)
            .bind(&base.repo_id)
            .fetch_one(&mut *tx)
            .await?;
            if !pinned {
                insert_base(&mut tx, base).await?;
            }
        }
        append_events(self, &mut tx, &input.events).await?;
        tx.commit().await?;
        Ok(record)
    }

    async fn get_project_review_policy(
        &self,
        project_id: &str,
    ) -> Result<Option<ProjectReviewPolicy>> {
        sqlx::query("SELECT * FROM project_review_policy WHERE project_id = ?")
            .bind(project_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_project_policy)
            .transpose()
    }

    async fn write_project_review_policy(
        &self,
        input: WriteProjectReviewPolicy,
    ) -> Result<ProjectReviewPolicy> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        require_project_admin(&mut tx, &input.project_id, &input.updated_by_user_id).await?;
        let current = sqlx::query("SELECT * FROM project_review_policy WHERE project_id = ?")
            .bind(&input.project_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_project_policy)
            .transpose()?;
        let (policy, digest) = match input.policy {
            Some((policy, digest)) => (Some(policy), Some(digest)),
            None => (None, None),
        };
        if current.as_ref().map(|record| record.version) != input.expected_version {
            let replay = current.filter(|record| {
                record.version == input.expected_version.unwrap_or(0) + 1
                    && record.policy == policy
                    && record.digest == digest
                    && record.updated_by_user_id == input.updated_by_user_id
            });
            return replay.ok_or(DbError::VersionConflict);
        }
        let policy_json = policy.as_ref().map(to_json).transpose()?;
        if current.is_some() {
            sqlx::query(
                "UPDATE project_review_policy
                 SET version = version + 1, policy_json = ?, digest = ?,
                     updated_by_user_id = ?, updated_at = ?
                 WHERE project_id = ?",
            )
            .bind(policy_json.as_deref())
            .bind(digest.as_deref())
            .bind(&input.updated_by_user_id)
            .bind(&input.updated_at)
            .bind(&input.project_id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        } else {
            sqlx::query(
                "INSERT INTO project_review_policy (
                     project_id, version, policy_json, digest, updated_by_user_id,
                     created_at, updated_at
                 ) VALUES (?, 1, ?, ?, ?, ?, ?)",
            )
            .bind(&input.project_id)
            .bind(policy_json.as_deref())
            .bind(digest.as_deref())
            .bind(&input.updated_by_user_id)
            .bind(&input.updated_at)
            .bind(&input.updated_at)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }
        let record = map_project_policy(
            sqlx::query("SELECT * FROM project_review_policy WHERE project_id = ?")
                .bind(&input.project_id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(record)
    }
}

async fn insert_base(tx: &mut Tx<'_>, base: &TaskReviewBase) -> Result<()> {
    sqlx::query(
        "INSERT INTO task_review_base (
             task_id, repo_id, revision, base_commit, base_tree, source, decision_id, pinned_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&base.task_id)
    .bind(&base.repo_id)
    .bind(base.revision)
    .bind(&base.base_commit)
    .bind(&base.base_tree)
    .bind(base.source.to_string())
    .bind(base.decision_id.as_deref())
    .bind(&base.pinned_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Checkpoints
// ---------------------------------------------------------------------------

fn merge_reasons(
    current: &[ReviewCheckpointReason],
    added: &[ReviewCheckpointReason],
) -> Vec<ReviewCheckpointReason> {
    let mut merged = current.to_vec();
    for reason in added {
        if !merged.contains(reason) {
            merged.push(*reason);
        }
    }
    merged
}

/// Cancels the open reviewer WorkUnits of a checkpoint. A running review is
/// never cancelled implicitly.
async fn cancel_review_work_units(
    tx: &mut Tx<'_>,
    checkpoint_id: &str,
    updated_at: &str,
) -> Result<()> {
    let running: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM work_unit_review_range r
             JOIN execution e ON e.work_unit_id = r.review_work_unit_id
             WHERE r.checkpoint_id = ? AND e.status = 'running')",
    )
    .bind(checkpoint_id)
    .fetch_one(&mut **tx)
    .await?;
    if running {
        return Err(DbError::InvalidTransition);
    }
    sqlx::query(
        "UPDATE work_unit SET status = 'cancelled', version = version + 1, updated_at = ?
         WHERE status = 'open' AND id IN (
             SELECT review_work_unit_id FROM work_unit_review_range WHERE checkpoint_id = ?)",
    )
    .bind(updated_at)
    .bind(checkpoint_id)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

/// Supersedes a `queued`, `blocked`, or never-executed `pending` checkpoint.
async fn supersede_checkpoint(
    tx: &mut Tx<'_>,
    id: &str,
    expected_version: i64,
    task_id: &str,
    updated_at: &str,
) -> Result<()> {
    let current = checkpoint_by_id(tx, id).await?.ok_or(DbError::NotFound)?;
    ensure(
        current.task_id == task_id,
        "superseded review checkpoint belongs to another Task",
    )?;
    if current.version != expected_version {
        return Err(DbError::VersionConflict);
    }
    match current.state {
        ReviewCheckpointState::Queued | ReviewCheckpointState::Blocked => {}
        ReviewCheckpointState::Pending => {
            let executed: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                     SELECT 1 FROM work_unit_review_range r
                     JOIN execution e ON e.work_unit_id = r.review_work_unit_id
                     WHERE r.checkpoint_id = ?)",
            )
            .bind(id)
            .fetch_one(&mut **tx)
            .await?;
            if executed {
                return Err(DbError::InvalidTransition);
            }
        }
        _ => return Err(DbError::InvalidTransition),
    }
    cancel_review_work_units(tx, id, updated_at).await?;
    let updated = sqlx::query(
        "UPDATE review_checkpoint
         SET state = 'superseded', blocked_code = NULL, blocked_detail_json = NULL,
             version = version + 1, updated_at = ?
         WHERE id = ? AND version = ?",
    )
    .bind(updated_at)
    .bind(id)
    .bind(expected_version)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    if updated.rows_affected() != 1 {
        return Err(DbError::VersionConflict);
    }
    Ok(())
}

async fn insert_checkpoint(tx: &mut Tx<'_>, checkpoint: &ReviewCheckpoint) -> Result<()> {
    sqlx::query(
        "INSERT INTO review_checkpoint (
             id, task_id, repo_id, state, reasons_json, attempt, base_commit, head_commit,
             range_id, policy_revision, blocked_code, blocked_detail_json, version,
             created_at, updated_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&checkpoint.id)
    .bind(&checkpoint.task_id)
    .bind(&checkpoint.repo_id)
    .bind(checkpoint.state.to_string())
    .bind(to_json(&checkpoint.reasons)?)
    .bind(checkpoint.attempt)
    .bind(checkpoint.base_commit.as_deref())
    .bind(&checkpoint.head_commit)
    .bind(checkpoint.range_id.as_deref())
    .bind(checkpoint.policy_revision)
    .bind(checkpoint.blocked_code.as_deref())
    .bind(checkpoint.blocked_detail_json.as_deref())
    .bind(checkpoint.version)
    .bind(&checkpoint.created_at)
    .bind(&checkpoint.updated_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

async fn record_checkpoint_request(
    tx: &mut Tx<'_>,
    checkpoint: &ReviewCheckpoint,
    request: Option<&ReviewCheckpointRequestRecord>,
    created_at: &str,
) -> Result<()> {
    let Some(request) = request else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO review_checkpoint_request (
             task_id, idempotency_key, checkpoint_id, source_kind, source_ref, reason, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&checkpoint.task_id)
    .bind(&request.idempotency_key)
    .bind(&checkpoint.id)
    .bind(request.source_kind.to_string())
    .bind(&request.source_ref)
    .bind(&request.reason)
    .bind(created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

async fn upsert_checkpoint_in_tx(
    db: &SqliteDb,
    tx: &mut Tx<'_>,
    input: UpsertQueuedReviewCheckpoint,
) -> Result<ReviewCheckpoint> {
    let mut checkpoint = input.checkpoint;
    ensure(
        matches!(
            checkpoint.state,
            ReviewCheckpointState::Queued | ReviewCheckpointState::Blocked
        ),
        "a new review checkpoint must be queued or blocked",
    )?;
    if let Some(request) = input.request.as_ref() {
        let existing = sqlx::query(
            "SELECT checkpoint_id, source_kind, source_ref, reason
             FROM review_checkpoint_request WHERE task_id = ? AND idempotency_key = ?",
        )
        .bind(&checkpoint.task_id)
        .bind(&request.idempotency_key)
        .fetch_optional(&mut **tx)
        .await?;
        if let Some(row) = existing {
            let id: String = row.try_get("checkpoint_id")?;
            let record = checkpoint_by_id(tx, &id).await?.ok_or(DbError::NotFound)?;
            if record.repo_id != checkpoint.repo_id
                || row.try_get::<String, _>("source_kind")? != request.source_kind.to_string()
                || row.try_get::<String, _>("source_ref")? != request.source_ref
                || row.try_get::<String, _>("reason")? != request.reason
            {
                return Err(DbError::IdempotencyConflict);
            }
            return Ok(record);
        }
    }
    if let Some(existing) = checkpoint_by_id(tx, &checkpoint.id).await? {
        if existing.task_id != checkpoint.task_id
            || existing.repo_id != checkpoint.repo_id
            || existing.head_commit != checkpoint.head_commit
            || existing.attempt != checkpoint.attempt
        {
            return Err(DbError::IdempotencyConflict);
        }
        return Ok(existing);
    }
    for (id, expected_version) in &input.supersede {
        supersede_checkpoint(
            tx,
            id,
            *expected_version,
            &checkpoint.task_id,
            &checkpoint.updated_at,
        )
        .await?;
    }
    let existing = sqlx::query(
        "SELECT * FROM review_checkpoint
         WHERE task_id = ? AND repo_id = ? AND head_commit = ? AND state = ?
           AND (state = 'queued' OR (attempt = ? AND blocked_code IS ?))
         ORDER BY created_at, id LIMIT 1",
    )
    .bind(&checkpoint.task_id)
    .bind(&checkpoint.repo_id)
    .bind(&checkpoint.head_commit)
    .bind(checkpoint.state.to_string())
    .bind(checkpoint.attempt)
    .bind(checkpoint.blocked_code.as_deref())
    .fetch_optional(&mut **tx)
    .await?
    .map(map_checkpoint)
    .transpose()?;
    if let Some(existing) = existing {
        let reasons = merge_reasons(&existing.reasons, &checkpoint.reasons);
        if checkpoint.attempt > existing.attempt {
            // Attempts are immutable: replace the queued row, keeping its reasons.
            supersede_checkpoint(
                tx,
                &existing.id,
                existing.version,
                &existing.task_id,
                &checkpoint.updated_at,
            )
            .await?;
            checkpoint.reasons = reasons;
        } else {
            if reasons != existing.reasons {
                let updated = sqlx::query(
                    "UPDATE review_checkpoint
                     SET reasons_json = ?, version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ?",
                )
                .bind(to_json(&reasons)?)
                .bind(&checkpoint.updated_at)
                .bind(&existing.id)
                .bind(existing.version)
                .execute(&mut **tx)
                .await
                .map_err(write_error)?;
                if updated.rows_affected() != 1 {
                    return Err(DbError::VersionConflict);
                }
            }
            record_checkpoint_request(
                tx,
                &existing,
                input.request.as_ref(),
                &checkpoint.updated_at,
            )
            .await?;
            // Events describing the unrealized new checkpoint are not emitted.
            for event in &input.events {
                if event.entity_id != checkpoint.id {
                    DomainEventRepo::append_event_in_tx(db, tx, event).await?;
                }
            }
            return checkpoint_by_id(tx, &existing.id)
                .await?
                .ok_or(DbError::NotFound);
        }
    }
    insert_checkpoint(tx, &checkpoint).await?;
    record_checkpoint_request(
        tx,
        &checkpoint,
        input.request.as_ref(),
        &checkpoint.created_at,
    )
    .await?;
    append_events(db, tx, &input.events).await?;
    checkpoint_by_id(tx, &checkpoint.id)
        .await?
        .ok_or(DbError::NotFound)
}

async fn insert_range(tx: &mut Tx<'_>, range: &ReviewRange) -> Result<()> {
    sqlx::query(
        "INSERT INTO review_range (
             id, task_id, repo_id, repository_set_revision, base_commit, head_commit,
             base_tree, head_tree, range_digest, integration_workspace_id,
             head_integration_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&range.id)
    .bind(&range.task_id)
    .bind(&range.repo_id)
    .bind(range.repository_set_revision)
    .bind(&range.base_commit)
    .bind(&range.head_commit)
    .bind(&range.base_tree)
    .bind(&range.head_tree)
    .bind(&range.range_digest)
    .bind(&range.integration_workspace_id)
    .bind(&range.head_integration_id)
    .bind(&range.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

#[async_trait]
impl ReviewCheckpointRepo for SqliteDb {
    async fn get_review_checkpoint(&self, id: &str) -> Result<Option<ReviewCheckpoint>> {
        let mut conn = self.pool.acquire().await?;
        checkpoint_by_id(&mut conn, id).await
    }

    async fn list_review_checkpoints(
        &self,
        task_id: &str,
        repo_id: Option<&str>,
        page: PageRequest,
    ) -> Result<Page<ReviewCheckpoint>> {
        let cursor = decode_phase_three_cursor(&page.cursor)?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("SELECT * FROM review_checkpoint WHERE task_id = ");
        query.push_bind(task_id);
        if let Some(repo_id) = repo_id {
            query.push(" AND repo_id = ").push_bind(repo_id);
        }
        if let Some(cursor) = cursor.as_ref() {
            query
                .push(" AND (created_at < ")
                .push_bind(&cursor.sort_value)
                .push(" OR (created_at = ")
                .push_bind(&cursor.sort_value)
                .push(" AND id < ")
                .push_bind(&cursor.id)
                .push("))");
        }
        query
            .push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit(&page) + 1);
        let items = query
            .build()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_checkpoint)
            .collect::<Result<Vec<_>>>()?;
        phase_three_page_from_items(
            items,
            &page,
            |item| item.created_at.clone(),
            |item| item.id.clone(),
        )
    }

    async fn upsert_queued_review_checkpoint(
        &self,
        input: UpsertQueuedReviewCheckpoint,
    ) -> Result<ReviewCheckpoint> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let record = upsert_checkpoint_in_tx(self, &mut tx, input).await?;
        tx.commit().await?;
        Ok(record)
    }

    async fn activate_review_checkpoint(
        &self,
        input: ActivateReviewCheckpoint,
    ) -> Result<ReviewCheckpoint> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let checkpoint = checkpoint_by_id(&mut tx, &input.checkpoint_id)
            .await?
            .ok_or(DbError::NotFound)?;
        if let Some(existing) = review_range_of_work_unit(&mut tx, &input.work_unit.id).await? {
            if existing.checkpoint_id != checkpoint.id
                || checkpoint.base_commit.as_deref() != Some(input.range.base_commit.as_str())
                || checkpoint.head_commit != input.range.head_commit
            {
                return Err(DbError::IdempotencyConflict);
            }
            return Ok(checkpoint);
        }
        if checkpoint.version != input.expected_version {
            return Err(DbError::VersionConflict);
        }
        if checkpoint.state != ReviewCheckpointState::Queued {
            return Err(DbError::InvalidTransition);
        }
        if coverage_revision(&mut tx, &checkpoint.task_id, &checkpoint.repo_id).await?
            != input.expected_coverage_revision
        {
            return Err(DbError::VersionConflict);
        }
        let range = &input.range;
        let frozen = &input.review_range;
        ensure(
            range.task_id == checkpoint.task_id
                && range.repo_id == checkpoint.repo_id
                && range.head_commit == checkpoint.head_commit
                && frozen.task_id == checkpoint.task_id
                && frozen.repo_id == checkpoint.repo_id
                && frozen.checkpoint_id == checkpoint.id
                && frozen.range_id == range.id
                && frozen.review_work_unit_id == input.work_unit.id
                && input.work_unit.task_id == checkpoint.task_id
                && !input.work_unit.requires_integration
                && input.work_unit.provenance.is_none()
                && input.work_unit.repository_ids.len() == 1
                && input.work_unit.repository_ids[0] == checkpoint.repo_id,
            "review checkpoint activation does not match its exact Task, repository and range",
        )?;
        let existing_range = sqlx::query(
            "SELECT * FROM review_range
             WHERE task_id = ? AND repo_id = ? AND base_commit = ? AND head_commit = ?",
        )
        .bind(&range.task_id)
        .bind(&range.repo_id)
        .bind(&range.base_commit)
        .bind(&range.head_commit)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_range)
        .transpose()?;
        let range_id = match existing_range {
            Some(existing) => {
                ensure(
                    existing.range_digest == range.range_digest
                        && existing.base_tree == range.base_tree
                        && existing.head_tree == range.head_tree,
                    "an immutable review range with these commits has a different identity",
                )?;
                existing.id
            }
            None => {
                insert_range(&mut tx, range).await?;
                range.id.clone()
            }
        };
        let contributors_frozen: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM work_unit_review_range WHERE range_id = ?)
                 OR EXISTS(SELECT 1 FROM review_range_contributor WHERE range_id = ?)",
        )
        .bind(&range_id)
        .bind(&range_id)
        .fetch_one(&mut *tx)
        .await?;
        if !contributors_frozen {
            for contributor in &input.contributors {
                ensure(
                    contributor.task_id == checkpoint.task_id && contributor.range_id == range.id,
                    "review contributor belongs to another range",
                )?;
                sqlx::query(
                    "INSERT INTO review_range_contributor (
                         id, task_id, range_id, work_unit_id, execution_id, attribution_id,
                         actor_kind, actor_id, created_at
                     ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                )
                .bind(&contributor.id)
                .bind(&contributor.task_id)
                .bind(&range_id)
                .bind(contributor.work_unit_id.as_deref())
                .bind(contributor.execution_id.as_deref())
                .bind(contributor.attribution_id.as_deref())
                .bind(contributor.actor.kind().to_string())
                .bind(contributor.actor.id())
                .bind(&contributor.created_at)
                .execute(&mut *tx)
                .await
                .map_err(write_error)?;
            }
        }
        let updated = sqlx::query(
            "UPDATE review_checkpoint
             SET state = 'pending', base_commit = ?, range_id = ?,
                 version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND state = 'queued'",
        )
        .bind(&range.base_commit)
        .bind(&range_id)
        .bind(&input.updated_at)
        .bind(&checkpoint.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if updated.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        super::work_unit::insert_work_unit_with_current_pin_in_tx(&mut tx, &input.work_unit)
            .await?;
        let mut input_findings = frozen.input_findings.clone();
        input_findings.sort_by(|left, right| left.finding_id.cmp(&right.finding_id));
        sqlx::query(
            "INSERT INTO work_unit_review_range (
                 review_work_unit_id, task_id, checkpoint_id, range_id, repo_id,
                 input_findings_json, input_findings_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&frozen.review_work_unit_id)
        .bind(&frozen.task_id)
        .bind(&frozen.checkpoint_id)
        .bind(&range_id)
        .bind(&frozen.repo_id)
        .bind(to_json(&input_findings)?)
        .bind(&frozen.input_findings_digest)
        .bind(&frozen.created_at)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        append_events(self, &mut tx, &input.events).await?;
        let record = checkpoint_by_id(&mut tx, &checkpoint.id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(record)
    }

    async fn transition_review_checkpoint(
        &self,
        input: TransitionReviewCheckpoint,
    ) -> Result<ReviewCheckpoint> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let current = checkpoint_by_id(&mut tx, &input.checkpoint_id)
            .await?
            .ok_or(DbError::NotFound)?;
        if current.version != input.expected_version {
            let replay = current.version == input.expected_version + 1
                && current.state == input.state
                && current.blocked_code == input.blocked_code
                && input
                    .add_reasons
                    .iter()
                    .all(|reason| current.reasons.contains(reason));
            return if replay {
                Ok(current)
            } else {
                Err(DbError::VersionConflict)
            };
        }
        ensure(
            (input.state == ReviewCheckpointState::Blocked) == input.blocked_code.is_some()
                && (input.state == ReviewCheckpointState::Blocked
                    || input.blocked_detail_json.is_none()),
            "only a blocked review checkpoint carries a blocked code",
        )?;
        let reasons = merge_reasons(&current.reasons, &input.add_reasons);
        let updated = sqlx::query(
            "UPDATE review_checkpoint
             SET state = ?, reasons_json = ?, blocked_code = ?, blocked_detail_json = ?,
                 version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(input.state.to_string())
        .bind(to_json(&reasons)?)
        .bind(input.blocked_code.as_deref())
        .bind(input.blocked_detail_json.as_deref())
        .bind(&input.updated_at)
        .bind(&current.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if updated.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        if input.cancel_review_work_units {
            cancel_review_work_units(&mut tx, &current.id, &input.updated_at).await?;
        }
        append_events(self, &mut tx, &input.events).await?;
        let record = checkpoint_by_id(&mut tx, &current.id)
            .await?
            .ok_or(DbError::NotFound)?;
        tx.commit().await?;
        Ok(record)
    }

    async fn get_review_range(&self, id: &str) -> Result<Option<ReviewRange>> {
        sqlx::query("SELECT * FROM review_range WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_range)
            .transpose()
    }

    async fn get_work_unit_review_range(
        &self,
        work_unit_id: &str,
    ) -> Result<Option<WorkUnitReviewRange>> {
        let mut conn = self.pool.acquire().await?;
        review_range_of_work_unit(&mut conn, work_unit_id).await
    }

    async fn list_review_range_contributors(
        &self,
        range_id: &str,
    ) -> Result<Vec<ReviewRangeContributor>> {
        sqlx::query(
            "SELECT * FROM review_range_contributor WHERE range_id = ?
             ORDER BY actor_kind, actor_id, id",
        )
        .bind(range_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_contributor)
        .collect()
    }

    async fn list_tasks_with_review_policy(
        &self,
        after_task_id: Option<&str>,
        limit: u32,
    ) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT p.task_id FROM task_review_policy_revision p
             JOIN task t ON t.id = p.task_id AND t.deleted_at IS NULL
             WHERE p.state = 'current' AND p.task_id > ?
             ORDER BY p.task_id LIMIT ?",
        )
        .bind(after_task_id.unwrap_or(""))
        .bind(i64::from(limit))
        .fetch_all(&self.pool)
        .await?)
    }

    async fn list_active_review_plan_checkpoints(
        &self,
        task_id: &str,
    ) -> Result<Vec<ReviewPlanCheckpoint>> {
        sqlx::query(
            "SELECT * FROM review_plan_checkpoint WHERE task_id = ? AND state = 'active'
             ORDER BY plan_artifact_id, plan_key",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_plan_checkpoint)
        .collect()
    }
}

// ---------------------------------------------------------------------------
// Range reports
// ---------------------------------------------------------------------------

fn same_report_artifact(existing: &Artifact, expected: &CreateArtifact) -> bool {
    existing.task_id == expected.task_id
        && existing.kind == expected.kind
        && existing.storage_kind == expected.storage_kind
        && existing.content == expected.content
        && existing.content_ref == expected.content_ref
        && existing.metadata_json == expected.metadata_json
        && existing.digest == expected.digest
        && existing.execution_producer().map(|(id, _)| id)
            == Some(expected.producer_execution_id.as_str())
}

async fn insert_finding(
    tx: &mut Tx<'_>,
    finding: &ReviewFinding,
    locations: &[ReviewFindingLocation],
) -> Result<()> {
    sqlx::query(
        "INSERT INTO review_finding (
             id, task_id, repo_id, range_id, report_artifact_id, ordinal, label, severity,
             title, evidence, invariant_text, anchor_commit, digest, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&finding.id)
    .bind(&finding.task_id)
    .bind(&finding.repo_id)
    .bind(&finding.range_id)
    .bind(&finding.report_artifact_id)
    .bind(finding.ordinal)
    .bind(&finding.label)
    .bind(finding.severity.to_string())
    .bind(&finding.title)
    .bind(&finding.evidence)
    .bind(finding.invariant.as_deref())
    .bind(&finding.anchor_commit)
    .bind(&finding.digest)
    .bind(&finding.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    for location in locations {
        ensure(
            location.finding_id == finding.id && location.task_id == finding.task_id,
            "finding location belongs to another finding",
        )?;
        let attribution = &location.attribution;
        sqlx::query(
            "INSERT INTO review_finding_location (
                 finding_id, task_id, ordinal, path, line_start, line_end, attribution_kind,
                 attributed_work_unit_id, attributed_execution_id, attributed_actor_kind,
                 attributed_actor_id, attributed_commit_sha, attributed_integration_id,
                 attributed_line_count
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&location.finding_id)
        .bind(&location.task_id)
        .bind(location.ordinal)
        .bind(&location.path)
        .bind(location.line_start)
        .bind(location.line_end)
        .bind(attribution.kind.to_string())
        .bind(attribution.work_unit_id.as_deref())
        .bind(attribution.execution_id.as_deref())
        .bind(
            attribution
                .actor
                .as_ref()
                .map(|actor| actor.kind().to_string()),
        )
        .bind(attribution.actor.as_ref().map(|actor| actor.id()))
        .bind(attribution.commit_sha.as_deref())
        .bind(attribution.integration_id.as_deref())
        .bind(attribution.line_count)
        .execute(&mut **tx)
        .await
        .map_err(write_error)?;
    }
    Ok(())
}

async fn insert_resolution(tx: &mut Tx<'_>, resolution: &ReviewFindingResolution) -> Result<()> {
    sqlx::query(
        "INSERT INTO review_finding_resolution (
             id, finding_id, task_id, repo_id, outcome, anchor_commit, report_artifact_id,
             decision_id, waiver_reference, evidence, actor_kind, actor_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&resolution.id)
    .bind(&resolution.finding_id)
    .bind(&resolution.task_id)
    .bind(&resolution.repo_id)
    .bind(resolution.outcome.to_string())
    .bind(&resolution.anchor_commit)
    .bind(resolution.report_artifact_id.as_deref())
    .bind(resolution.decision_id.as_deref())
    .bind(resolution.waiver_reference.as_deref())
    .bind(&resolution.evidence)
    .bind(resolution.actor.kind().to_string())
    .bind(resolution.actor.id())
    .bind(&resolution.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    Ok(())
}

#[async_trait]
impl ReviewRangeReportRepo for SqliteDb {
    async fn ingest_range_review_report(
        &self,
        artifact: CreateArtifact,
        evidence_input_ids: Vec<String>,
        artifact_input_ids: Vec<String>,
        artifact_event: CreateDomainEvent,
        range: IngestRangeReport,
    ) -> Result<ExecutionArtifactOutputWrite> {
        let report = &range.report;
        ensure(
            artifact.kind == ArtifactKind::ReviewReport
                && artifact.storage_kind == ArtifactStorageKind::Inline
                && artifact.content.is_some()
                && artifact.content_ref.is_none()
                && report.report_artifact_id == artifact.id
                && report.task_id == artifact.task_id
                && report.execution_id == artifact.producer_execution_id
                && artifact.digest.as_deref() == Some(report.report_digest.as_str()),
            "range review requires its exact inline ReviewReport",
        )?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing_id: Option<String> = sqlx::query_scalar(
            "SELECT artifact_id FROM execution_artifact_output
             WHERE execution_id = ? AND kind = 'review_report'",
        )
        .bind(&artifact.producer_execution_id)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(existing_id) = existing_id {
            let ledger: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM review_range_report
                 WHERE report_artifact_id = ? AND checkpoint_id = ?)",
            )
            .bind(&existing_id)
            .bind(&report.checkpoint_id)
            .fetch_one(&mut *tx)
            .await?;
            tx.rollback().await?;
            let existing = CollaborationRepo::get_execution_artifact_output(
                self,
                &artifact.producer_execution_id,
                ArtifactKind::ReviewReport,
            )
            .await?
            .ok_or(DbError::VersionConflict)?;
            if !ledger || !same_report_artifact(&existing, &artifact) {
                return Err(invalid(
                    "Execution already has a different output Artifact of this kind",
                ));
            }
            return Ok(ExecutionArtifactOutputWrite {
                artifact: existing,
                event: None,
            });
        }

        let checkpoint = checkpoint_by_id(&mut tx, &report.checkpoint_id)
            .await?
            .ok_or(DbError::NotFound)?;
        if checkpoint.version != range.expected_checkpoint_version
            || !matches!(
                checkpoint.state,
                ReviewCheckpointState::Pending | ReviewCheckpointState::InReview
            )
        {
            return Err(DbError::VersionConflict);
        }
        let frozen = review_range_of_work_unit(&mut tx, &report.review_work_unit_id)
            .await?
            .ok_or(DbError::NotFound)?;
        ensure(
            frozen.checkpoint_id == checkpoint.id
                && frozen.range_id == report.range_id
                && frozen.repo_id == report.repo_id
                && frozen.task_id == report.task_id,
            "range report subject does not match its frozen WorkUnit range",
        )?;
        // Current reviewer authority, over the same facts as every ReviewReport.
        let authorized: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM execution e
                 JOIN execution_task_role_authority authority ON authority.execution_id = e.id
                 JOIN task_role role
                   ON role.id = authority.task_role_id AND role.task_id = e.task_id
                 JOIN task_role_capability_configuration configuration
                   ON configuration.task_role_id = role.id
                 JOIN role_membership membership
                   ON membership.task_role_id = role.id
                  AND membership.actor_kind = e.actor_kind AND membership.actor_id = e.actor_id
                 JOIN json_each(authority.capabilities_json) capability
                   ON capability.type = 'text' AND capability.value = 'formal_review'
                 WHERE e.id = ? AND e.task_id = ? AND e.work_unit_id = ?
                   AND e.actor_kind = ? AND e.actor_id = ?
                   AND membership.status = 'active'
                   AND json_extract(configuration.configuration_json, '$.schema_version') = 1
                   AND EXISTS (
                       SELECT 1 FROM json_each(configuration.configuration_json, '$.capabilities') c
                       WHERE c.type = 'text' AND c.value = 'formal_review')
                   AND (e.status = 'running'
                        OR (e.actor_kind = 'agent' AND e.status = 'completed'))
             )",
        )
        .bind(&report.execution_id)
        .bind(&report.task_id)
        .bind(&report.review_work_unit_id)
        .bind(report.actor.kind().to_string())
        .bind(report.actor.id())
        .fetch_one(&mut *tx)
        .await?;
        ensure(
            authorized,
            "range ReviewReport requires current authoritative reviewer membership and its exact formal_review Execution",
        )?;
        let dependent: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM review_range_contributor x
                 JOIN execution e ON e.id = ?
                 WHERE x.range_id = ? AND x.actor_kind = e.actor_kind
                   AND x.actor_id = e.actor_id
                   AND (e.actor_kind = 'human' OR e.harness_session_id IS NULL))",
        )
        .bind(&report.execution_id)
        .bind(&report.range_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure(!dependent, "range reviewer is not independent of the range")?;

        let next = range.next_checkpoint_state;
        let questions = report.verdict == ReviewVerdict::Questions;
        ensure(
            matches!(
                next,
                ReviewCheckpointState::InReview
                    | ReviewCheckpointState::Reviewed
                    | ReviewCheckpointState::Blocked
            ) && (next == ReviewCheckpointState::Blocked) == range.blocked_code.is_some()
                && (!questions
                    || (next == ReviewCheckpointState::Blocked
                        && range.findings.is_empty()
                        && range.resolutions.is_empty())),
            "range report requests an invalid checkpoint transition",
        )?;
        if next == ReviewCheckpointState::Reviewed {
            let required = current_policy(&mut tx, &report.task_id)
                .await?
                .map_or(1, |policy| i64::from(policy.policy.required_reviewers));
            let others: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM (
                     SELECT DISTINCT actor_kind, actor_id FROM review_range_report
                     WHERE checkpoint_id = ? AND verdict != 'questions'
                       AND NOT (actor_kind = ? AND actor_id = ?))",
            )
            .bind(&checkpoint.id)
            .bind(report.actor.kind().to_string())
            .bind(report.actor.id())
            .fetch_one(&mut *tx)
            .await?;
            ensure(
                others + 1 >= required,
                "review checkpoint quorum is not met",
            )?;
        }

        if report.actor.kind() == ActorKind::Human
            || !evidence_input_ids.is_empty()
            || !artifact_input_ids.is_empty()
        {
            super::collaboration::validate_review_report_references(
                artifact.content.as_deref().unwrap_or_default(),
                &evidence_input_ids,
                &artifact_input_ids,
            )?;
        }
        for evidence_id in &evidence_input_ids {
            super::validation_run::pin_execution_evidence_input_in_tx(
                &mut tx,
                &artifact.producer_execution_id,
                evidence_id,
                &artifact.created_at,
            )
            .await?;
        }
        for artifact_id in &artifact_input_ids {
            super::collaboration::pin_execution_artifact_input_in_tx(
                &mut tx,
                &artifact.producer_execution_id,
                artifact_id,
                &artifact.created_at,
            )
            .await?;
        }
        sqlx::query(
            "INSERT INTO artifact_execution_producer (artifact_id, execution_id, task_id)
             VALUES (?, ?, ?)",
        )
        .bind(&artifact.id)
        .bind(&artifact.producer_execution_id)
        .bind(&artifact.task_id)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        sqlx::query(
            "INSERT INTO execution_artifact_output
             (execution_id, artifact_id, task_id, kind, digest, created_at)
             VALUES (?, ?, ?, 'review_report', ?, ?)",
        )
        .bind(&artifact.producer_execution_id)
        .bind(&artifact.id)
        .bind(&artifact.task_id)
        .bind(&artifact.digest)
        .bind(&artifact.created_at)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        sqlx::query(
            "INSERT INTO artifact (
                id, task_id, kind, storage_kind, content, content_ref,
                metadata_json, digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&artifact.id)
        .bind(&artifact.task_id)
        .bind(artifact.kind.to_string())
        .bind(artifact.storage_kind.to_string())
        .bind(&artifact.content)
        .bind(&artifact.content_ref)
        .bind(&artifact.metadata_json)
        .bind(&artifact.digest)
        .bind(&artifact.created_at)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        sqlx::query(
            "INSERT INTO review_range_report (
                 report_artifact_id, task_id, repo_id, range_id, checkpoint_id,
                 review_work_unit_id, execution_id, actor_kind, actor_id, verdict,
                 report_digest, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&report.report_artifact_id)
        .bind(&report.task_id)
        .bind(&report.repo_id)
        .bind(&report.range_id)
        .bind(&report.checkpoint_id)
        .bind(&report.review_work_unit_id)
        .bind(&report.execution_id)
        .bind(report.actor.kind().to_string())
        .bind(report.actor.id())
        .bind(report.verdict.to_string())
        .bind(&report.report_digest)
        .bind(&report.created_at)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        for (finding, locations) in &range.findings {
            ensure(
                finding.report_artifact_id == artifact.id,
                "finding belongs to another range report",
            )?;
            insert_finding(&mut tx, finding, locations).await?;
        }
        for resolution in &range.resolutions {
            ensure(
                resolution.report_artifact_id.as_deref() == Some(artifact.id.as_str())
                    && resolution.outcome != ReviewResolutionOutcome::Waived,
                "a range report resolves findings only through itself",
            )?;
            insert_resolution(&mut tx, resolution).await?;
        }
        let updated = sqlx::query(
            "UPDATE review_checkpoint
             SET state = ?, blocked_code = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(next.to_string())
        .bind(range.blocked_code.as_deref())
        .bind(&report.created_at)
        .bind(&checkpoint.id)
        .bind(range.expected_checkpoint_version)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if updated.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        let event = DomainEventRepo::append_event_in_tx(self, &mut tx, &artifact_event).await?;
        append_events(self, &mut tx, &range.events).await?;
        tx.commit().await?;
        let record = super::collaboration::get_artifact_row(self, &artifact.id)
            .await?
            .ok_or(DbError::NotFound)?;
        Ok(ExecutionArtifactOutputWrite {
            artifact: record,
            event: Some(event),
        })
    }

    async fn get_review_range_report(
        &self,
        artifact_id: &str,
    ) -> Result<Option<ReviewRangeReport>> {
        sqlx::query("SELECT * FROM review_range_report WHERE report_artifact_id = ?")
            .bind(artifact_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_report)
            .transpose()
    }
}

// ---------------------------------------------------------------------------
// Ledger
// ---------------------------------------------------------------------------

/// Inserts an approved Human Proposal and Decision after revalidating current
/// authority. Waivers allow active Task members; re-baselines and attributions
/// remain restricted to Project owners/admins.
async fn insert_human_decision(
    tx: &mut Tx<'_>,
    task_id: &str,
    target_kind: ProposalTargetKind,
    target_id: &str,
    input: &ReviewHumanDecision,
) -> Result<()> {
    let proposal = &input.proposal;
    let decision = &input.decision;
    ensure(
        proposal.task_id == task_id
            && decision.task_id == task_id
            && proposal.target.kind == target_kind
            && proposal.target.id == target_id
            && decision.proposal_id == proposal.id
            && decision.proposal_version == 1
            && decision.outcome == DecisionOutcome::Approve
            && !decision.actors.is_empty()
            && decision
                .actors
                .iter()
                .all(|actor| actor.kind() == ActorKind::Human),
        "review authority change requires its exact approved Human Proposal and Decision",
    )?;
    let project_id = task_project_id(tx, task_id).await?;
    for actor in &decision.actors {
        if target_kind == ProposalTargetKind::ReviewFinding {
            require_review_waiver_authority(tx, task_id, actor.id()).await?;
        } else {
            require_project_admin(tx, &project_id, actor.id()).await?;
        }
    }
    for artifact_id in &proposal.artifact_ids {
        sqlx::query(
            "INSERT INTO proposal_artifact (proposal_id, artifact_id, task_id) VALUES (?, ?, ?)",
        )
        .bind(&proposal.id)
        .bind(artifact_id)
        .bind(task_id)
        .execute(&mut **tx)
        .await
        .map_err(write_error)?;
    }
    sqlx::query(
        "INSERT INTO proposal (
            id, task_id, proposer_actor_kind, proposer_actor_id, target_kind, target_id,
            action, reason, target_version, target_digest, required_policy_ref,
            required_policy_version, required_policy_digest, content_version, status,
            supersedes_proposal_id, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, 'open', ?, ?)",
    )
    .bind(&proposal.id)
    .bind(task_id)
    .bind(proposal.proposer.kind().to_string())
    .bind(proposal.proposer.id())
    .bind(proposal.target.kind.to_string())
    .bind(&proposal.target.id)
    .bind(&proposal.action)
    .bind(&proposal.reason)
    .bind(proposal.target_version)
    .bind(proposal.target_digest.as_deref())
    .bind(proposal.required_policy_ref.as_deref())
    .bind(proposal.required_policy_version)
    .bind(proposal.required_policy_digest.as_deref())
    .bind(proposal.supersedes_proposal_id.as_deref())
    .bind(&proposal.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    for actor in &decision.actors {
        sqlx::query(
            "INSERT INTO decision_actor (decision_id, actor_kind, actor_id, task_id)
             VALUES (?, ?, ?, ?)",
        )
        .bind(&decision.id)
        .bind(actor.kind().to_string())
        .bind(actor.id())
        .bind(task_id)
        .execute(&mut **tx)
        .await
        .map_err(write_error)?;
    }
    sqlx::query(
        "INSERT INTO decision (
            id, task_id, proposal_id, proposal_version, outcome, rationale,
            policy_ref, policy_version, policy_digest, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&decision.id)
    .bind(task_id)
    .bind(&decision.proposal_id)
    .bind(decision.proposal_version)
    .bind(decision.outcome.to_string())
    .bind(&decision.rationale)
    .bind(decision.policy_ref.as_deref())
    .bind(decision.policy_version)
    .bind(decision.policy_digest.as_deref())
    .bind(&decision.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    let resolved = sqlx::query(
        "UPDATE proposal SET status = 'resolved'
         WHERE id = ? AND task_id = ? AND content_version = 1 AND status = 'open'",
    )
    .bind(&proposal.id)
    .bind(task_id)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    if resolved.rows_affected() != 1 {
        return Err(DbError::VersionConflict);
    }
    Ok(())
}

async fn round_for_report(
    tx: &mut Tx<'_>,
    report_artifact_id: &str,
) -> Result<Option<ReviewReworkRound>> {
    sqlx::query("SELECT * FROM review_rework_round WHERE report_artifact_id = ?")
        .bind(report_artifact_id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_round)
        .transpose()
}

/// Round, correction WorkUnits, dependencies, links and events, in `tx`.
async fn apply_corrections_in_tx(
    db: &SqliteDb,
    tx: &mut Tx<'_>,
    input: &ApplyReviewCorrections,
) -> Result<ReviewReworkRound> {
    let round = &input.round;
    let state: Option<String> = sqlx::query_scalar(
        "SELECT l.state FROM task t JOIN task_lifecycle l ON l.task_id = t.id
         WHERE t.id = ? AND t.deleted_at IS NULL",
    )
    .bind(&round.task_id)
    .fetch_optional(&mut **tx)
    .await?;
    match state.as_deref() {
        None => return Err(DbError::NotFound),
        Some("done" | "cancelled") => return Err(DbError::InvalidTransition),
        Some(_) => {}
    }
    let report_in_task: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM review_range_report
         WHERE report_artifact_id = ? AND task_id = ?)",
    )
    .bind(&round.report_artifact_id)
    .bind(&round.task_id)
    .fetch_one(&mut **tx)
    .await?;
    ensure(
        report_in_task,
        "rework round requires this Task's range report",
    )?;
    let consumed: i64 = sqlx::query_scalar(
        "SELECT (SELECT COUNT(*) FROM task_rework_receipt WHERE task_id = ?)
              + (SELECT COUNT(*) FROM review_rework_round WHERE task_id = ?)
              + (SELECT COUNT(*) FROM task_review_triage triage WHERE triage.task_id = ?
                   AND (triage.disposition = 'recheck' OR EXISTS (
                       SELECT 1 FROM review_finding_triage finding_triage
                       WHERE finding_triage.report_artifact_id = triage.review_report_artifact_id
                         AND finding_triage.disposition = 'recheck'
                   )))",
    )
    .bind(&round.task_id)
    .bind(&round.task_id)
    .bind(&round.task_id)
    .fetch_one(&mut **tx)
    .await?;
    ensure(
        consumed < input.max_rework_rounds && round.round == consumed + 1,
        "review.rework.round_budget_exhausted",
    )?;
    sqlx::query(
        "INSERT INTO review_rework_round (
             report_artifact_id, task_id, round, request_id, created_by_kind,
             created_by_id, idempotency_key, created_at
         ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&round.report_artifact_id)
    .bind(&round.task_id)
    .bind(round.round)
    .bind(round.request_id.as_deref())
    .bind(round.created_by.kind().to_string())
    .bind(round.created_by.id())
    .bind(&round.idempotency_key)
    .bind(&round.created_at)
    .execute(&mut **tx)
    .await
    .map_err(write_error)?;
    let created = |id: &str| input.work_units.iter().any(|unit| unit.id == id);
    for work_unit in &input.work_units {
        ensure(
            work_unit.task_id == round.task_id,
            "correction WorkUnit belongs to another Task",
        )?;
        super::work_unit::insert_work_unit_with_current_pin_in_tx(tx, work_unit).await?;
    }
    for dependency in &input.dependencies {
        ensure(
            created(&dependency.work_unit_id),
            "correction dependencies may only constrain new correction WorkUnits",
        )?;
        super::work_unit::add_dependency_in_tx(tx, dependency).await?;
    }
    for link in &input.links {
        ensure(
            link.task_id == round.task_id
                && link.report_artifact_id == round.report_artifact_id
                && created(&link.correction_work_unit_id),
            "correction link must join this round's WorkUnit and report",
        )?;
        let finding_in_scope: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM review_finding
             WHERE id = ? AND task_id = ? AND repo_id = ?)",
        )
        .bind(&link.finding_id)
        .bind(&link.task_id)
        .bind(&link.repo_id)
        .fetch_one(&mut **tx)
        .await?;
        ensure(
            finding_in_scope,
            "correction link finding belongs to another Task or repository",
        )?;
        sqlx::query(
            "INSERT INTO review_correction_link (
                 correction_work_unit_id, finding_id, task_id, repo_id,
                 report_artifact_id, base_commit, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&link.correction_work_unit_id)
        .bind(&link.finding_id)
        .bind(&link.task_id)
        .bind(&link.repo_id)
        .bind(&link.report_artifact_id)
        .bind(&link.base_commit)
        .bind(&link.created_at)
        .execute(&mut **tx)
        .await
        .map_err(write_error)?;
    }
    append_events(db, tx, &input.events).await?;
    Ok(round.clone())
}

#[async_trait]
impl ReviewLedgerRepo for SqliteDb {
    async fn load_review_coverage_rows(
        &self,
        task_id: &str,
        repo_id: &str,
    ) -> Result<ReviewCoverageRows> {
        let mut tx = self.pool.begin().await?;
        let rows = self
            .load_review_coverage_rows_in_tx(&mut tx, task_id, repo_id)
            .await?;
        tx.commit().await?;
        Ok(rows)
    }

    async fn load_review_coverage_rows_in_tx(
        &self,
        conn: &mut SqliteConnection,
        task_id: &str,
        repo_id: &str,
    ) -> Result<ReviewCoverageRows> {
        let policy = current_policy(conn, task_id).await?;
        let base = sqlx::query(
            "SELECT * FROM task_review_base WHERE task_id = ? AND repo_id = ?
             ORDER BY revision DESC LIMIT 1",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_optional(&mut *conn)
        .await?
        .map(map_base)
        .transpose()?;
        let state_revision = coverage_revision(conn, task_id, repo_id).await?;
        let integrations = sqlx::query(
            "SELECT * FROM work_unit_repository_integration WHERE task_id = ? AND repo_id = ?
             ORDER BY started_at, id",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(super::work_unit_repository_integration::map_repository_integration)
        .collect::<Result<Vec<_>>>()?;
        let ranges = sqlx::query(
            "SELECT * FROM review_range WHERE task_id = ? AND repo_id = ?
             ORDER BY created_at, id",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(map_range)
        .collect::<Result<Vec<_>>>()?;
        let checkpoints = sqlx::query(
            "SELECT * FROM review_checkpoint WHERE task_id = ? AND repo_id = ?
             ORDER BY created_at, id",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(map_checkpoint)
        .collect::<Result<Vec<_>>>()?;
        let reports = sqlx::query(
            "SELECT * FROM review_range_report WHERE task_id = ? AND repo_id = ?
             ORDER BY created_at, report_artifact_id",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(map_report)
        .collect::<Result<Vec<_>>>()?;
        let findings = sqlx::query(
            "SELECT * FROM review_finding WHERE task_id = ? AND repo_id = ?
             ORDER BY created_at, report_artifact_id, ordinal",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(map_finding)
        .collect::<Result<Vec<_>>>()?;
        let resolutions = sqlx::query(
            "SELECT * FROM review_finding_resolution WHERE task_id = ? AND repo_id = ?
             ORDER BY created_at, id",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(map_resolution)
        .collect::<Result<Vec<_>>>()?;
        let attributions = sqlx::query(
            "SELECT * FROM review_commit_attribution WHERE task_id = ? AND repo_id = ?
             ORDER BY commit_sha",
        )
        .bind(task_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?
        .into_iter()
        .map(map_attribution)
        .collect::<Result<Vec<_>>>()?;
        // A line touches the repository through its current result, its explicit
        // WorkUnit scope, or an unscoped integrating WorkUnit (all writable repos).
        let failing_result_lines = sqlx::query_scalar(
            "SELECT line.id FROM work_unit_review_line line
             WHERE line.task_id = ? AND line.latest_verdict = 'request_changes'
               AND (
                   EXISTS (SELECT 1 FROM execution_repository_result result
                           WHERE result.execution_id = line.current_result_execution_id
                             AND result.repo_id = ?)
                   OR EXISTS (SELECT 1 FROM work_unit_repository scope
                              WHERE scope.work_unit_id = line.current_work_unit_id
                                AND scope.repo_id = ?)
                   OR EXISTS (SELECT 1 FROM work_unit unit
                              WHERE unit.id = line.current_work_unit_id
                                AND unit.requires_integration = 1
                                AND NOT EXISTS (SELECT 1 FROM work_unit_repository scope
                                                WHERE scope.work_unit_id = unit.id))
               )
             ORDER BY line.id",
        )
        .bind(task_id)
        .bind(repo_id)
        .bind(repo_id)
        .fetch_all(&mut *conn)
        .await?;
        Ok(ReviewCoverageRows {
            policy,
            base,
            state_revision,
            integrations,
            ranges,
            checkpoints,
            reports,
            findings,
            resolutions,
            attributions,
            failing_result_lines,
        })
    }

    async fn list_review_findings(
        &self,
        task_id: &str,
        filter: ReviewFindingFilter,
        page: PageRequest,
    ) -> Result<Page<ReviewFinding>> {
        let cursor = decode_phase_three_cursor(&page.cursor)?;
        let mut query =
            sqlx::QueryBuilder::<Sqlite>::new("SELECT * FROM review_finding WHERE task_id = ");
        query.push_bind(task_id);
        if let Some(repo_id) = filter.repo_id.as_deref() {
            query.push(" AND repo_id = ").push_bind(repo_id);
        }
        if let Some(severity) = filter.severity {
            query
                .push(" AND severity = ")
                .push_bind(severity.to_string());
        }
        if let Some(finding_ids) = filter.finding_ids.as_ref() {
            query
                .push(" AND id IN (SELECT value FROM json_each(")
                .push_bind(to_json(finding_ids)?)
                .push("))");
        }
        if let Some(cursor) = cursor.as_ref() {
            query
                .push(" AND (created_at < ")
                .push_bind(&cursor.sort_value)
                .push(" OR (created_at = ")
                .push_bind(&cursor.sort_value)
                .push(" AND id < ")
                .push_bind(&cursor.id)
                .push("))");
        }
        query
            .push(" ORDER BY created_at DESC, id DESC LIMIT ")
            .push_bind(limit(&page) + 1);
        let items = query
            .build()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_finding)
            .collect::<Result<Vec<_>>>()?;
        phase_three_page_from_items(
            items,
            &page,
            |item| item.created_at.clone(),
            |item| item.id.clone(),
        )
    }

    async fn get_review_finding(&self, id: &str) -> Result<Option<ReviewFinding>> {
        let mut conn = self.pool.acquire().await?;
        finding_by_id(&mut conn, id).await
    }

    async fn list_review_finding_locations(
        &self,
        finding_id: &str,
    ) -> Result<Vec<ReviewFindingLocation>> {
        sqlx::query("SELECT * FROM review_finding_location WHERE finding_id = ? ORDER BY ordinal")
            .bind(finding_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_location)
            .collect()
    }

    async fn list_review_finding_resolutions(
        &self,
        finding_id: &str,
    ) -> Result<Vec<ReviewFindingResolution>> {
        sqlx::query(
            "SELECT * FROM review_finding_resolution WHERE finding_id = ?
             ORDER BY created_at, id",
        )
        .bind(finding_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_resolution)
        .collect()
    }

    async fn list_review_finding_triage(
        &self,
        finding_id: &str,
    ) -> Result<Vec<ReviewFindingTriage>> {
        sqlx::query(
            "SELECT * FROM review_finding_triage WHERE finding_id = ?
             ORDER BY created_at, request_id",
        )
        .bind(finding_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_triage)
        .collect()
    }

    async fn list_review_correction_links(
        &self,
        finding_id: &str,
    ) -> Result<Vec<ReviewCorrectionLink>> {
        sqlx::query(
            "SELECT * FROM review_correction_link WHERE finding_id = ?
             ORDER BY created_at, correction_work_unit_id",
        )
        .bind(finding_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_link)
        .collect()
    }

    async fn waive_review_finding(
        &self,
        expected_coverage_revision: i64,
        decision: ReviewHumanDecision,
        resolution: ReviewFindingResolution,
        events: Vec<CreateDomainEvent>,
    ) -> Result<ReviewFindingResolution> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query("SELECT * FROM review_finding_resolution WHERE id = ?")
            .bind(&resolution.id)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_resolution)
            .transpose()?
        {
            return if existing == resolution {
                Ok(existing)
            } else {
                Err(DbError::IdempotencyConflict)
            };
        }
        let finding = finding_by_id(&mut tx, &resolution.finding_id)
            .await?
            .ok_or(DbError::NotFound)?;
        ensure(
            finding.severity != ReviewFindingSeverity::P0,
            "review.finding.not_waivable",
        )?;
        ensure(
            resolution.outcome == ReviewResolutionOutcome::Waived
                && resolution.task_id == finding.task_id
                && resolution.repo_id == finding.repo_id
                && resolution.report_artifact_id.is_none()
                && resolution.decision_id.as_deref() == Some(decision.decision.id.as_str())
                && decision.decision.actors.contains(&resolution.actor)
                && decision.proposal.target_digest.as_deref() == Some(finding.digest.as_str())
                && resolution
                    .waiver_reference
                    .as_deref()
                    .is_some_and(|reference| !reference.trim().is_empty()),
            "waiver must bind its exact finding and Human Decision",
        )?;
        if coverage_revision(&mut tx, &finding.task_id, &finding.repo_id).await?
            != expected_coverage_revision
        {
            return Err(DbError::VersionConflict);
        }
        insert_human_decision(
            &mut tx,
            &finding.task_id,
            ProposalTargetKind::ReviewFinding,
            &finding.id,
            &decision,
        )
        .await?;
        insert_resolution(&mut tx, &resolution).await?;
        append_events(self, &mut tx, &events).await?;
        tx.commit().await?;
        Ok(resolution)
    }

    async fn pin_review_base(
        &self,
        base: TaskReviewBase,
        events: Vec<CreateDomainEvent>,
    ) -> Result<TaskReviewBase> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure(
            base.revision == 1
                && base.decision_id.is_none()
                && matches!(
                    base.source,
                    crate::ReviewBaseSource::FirstIntegration
                        | crate::ReviewBaseSource::WorkspacePrepare
                ),
            "initial review base must be revision 1 from first integration or workspace preparation",
        )?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_review_base WHERE task_id = ? AND repo_id = ?
             ORDER BY revision LIMIT 1",
        )
        .bind(&base.task_id)
        .bind(&base.repo_id)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_base)
        .transpose()?
        {
            return if existing == base {
                Ok(existing)
            } else {
                Err(DbError::VersionConflict)
            };
        }
        insert_base(&mut tx, &base).await?;
        append_events(self, &mut tx, &events).await?;
        tx.commit().await?;
        Ok(base)
    }

    async fn rebaseline_review(
        &self,
        expected_coverage_revision: i64,
        decision: ReviewHumanDecision,
        base: TaskReviewBase,
        events: Vec<CreateDomainEvent>,
    ) -> Result<TaskReviewBase> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query(
            "SELECT * FROM task_review_base WHERE task_id = ? AND repo_id = ? AND revision = ?",
        )
        .bind(&base.task_id)
        .bind(&base.repo_id)
        .bind(base.revision)
        .fetch_optional(&mut *tx)
        .await?
        .map(map_base)
        .transpose()?
        {
            return if existing == base {
                Ok(existing)
            } else {
                Err(DbError::VersionConflict)
            };
        }
        if coverage_revision(&mut tx, &base.task_id, &base.repo_id).await?
            != expected_coverage_revision
        {
            return Err(DbError::VersionConflict);
        }
        ensure(
            base.source == crate::ReviewBaseSource::HumanRebaseline
                && base.decision_id.as_deref() == Some(decision.decision.id.as_str()),
            "re-baseline requires its exact Human Decision",
        )?;
        insert_human_decision(
            &mut tx,
            &base.task_id,
            ProposalTargetKind::ReviewBase,
            &base.repo_id,
            &decision,
        )
        .await?;
        insert_base(&mut tx, &base).await?;
        append_events(self, &mut tx, &events).await?;
        tx.commit().await?;
        Ok(base)
    }

    async fn attribute_review_commits(
        &self,
        checkpoint_id: &str,
        expected_version: i64,
        decision: ReviewHumanDecision,
        attributions: Vec<ReviewCommitAttribution>,
        events: Vec<CreateDomainEvent>,
    ) -> Result<ReviewCheckpoint> {
        ensure(
            !attributions.is_empty(),
            "commit attribution requires at least one commit",
        )?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let checkpoint = checkpoint_by_id(&mut tx, checkpoint_id)
            .await?
            .ok_or(DbError::NotFound)?;
        let mut replayed = 0;
        for attribution in &attributions {
            let existing = sqlx::query("SELECT * FROM review_commit_attribution WHERE id = ?")
                .bind(&attribution.id)
                .fetch_optional(&mut *tx)
                .await?
                .map(map_attribution)
                .transpose()?;
            if existing.as_ref() == Some(attribution) {
                replayed += 1;
            }
        }
        if replayed == attributions.len() {
            return Ok(checkpoint);
        }
        if checkpoint.version != expected_version {
            return Err(DbError::VersionConflict);
        }
        if checkpoint.state != ReviewCheckpointState::Blocked {
            return Err(DbError::InvalidTransition);
        }
        insert_human_decision(
            &mut tx,
            &checkpoint.task_id,
            ProposalTargetKind::ReviewCommit,
            &checkpoint.repo_id,
            &decision,
        )
        .await?;
        for attribution in &attributions {
            ensure(
                attribution.task_id == checkpoint.task_id
                    && attribution.repo_id == checkpoint.repo_id
                    && attribution.decision_id == decision.decision.id,
                "commit attribution must bind this checkpoint's repository and Decision",
            )?;
            sqlx::query(
                "INSERT INTO review_commit_attribution (
                     id, task_id, repo_id, commit_sha, actor_kind, actor_id,
                     decision_id, created_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&attribution.id)
            .bind(&attribution.task_id)
            .bind(&attribution.repo_id)
            .bind(&attribution.commit_sha)
            .bind(attribution.actor.kind().to_string())
            .bind(attribution.actor.id())
            .bind(&attribution.decision_id)
            .bind(&attribution.created_at)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }
        append_events(self, &mut tx, &events).await?;
        tx.commit().await?;
        Ok(checkpoint)
    }

    async fn apply_review_triage(&self, input: ApplyReviewTriage) -> Result<()> {
        let aggregate = &input.aggregate;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT decision_digest FROM task_review_triage WHERE review_report_artifact_id = ?",
        )
        .bind(&aggregate.review_report_artifact_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            return if existing == aggregate.decision_digest {
                Ok(())
            } else {
                Err(DbError::IdempotencyConflict)
            };
        }
        let range_report: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM review_range_report
             WHERE report_artifact_id = ? AND task_id = ?)",
        )
        .bind(&aggregate.review_report_artifact_id)
        .bind(&aggregate.task_id)
        .fetch_one(&mut *tx)
        .await?;
        ensure(
            range_report,
            "range triage requires this Task's range report",
        )?;
        let request = sqlx::query(
            "SELECT id, version FROM task_coordination_request
             WHERE task_id = ? AND kind = 'review_findings' AND review_report_artifact_id = ?
               AND authorization_version = ? AND execution_id = ?
               AND state IN ('running', 'uncertain')",
        )
        .bind(&aggregate.task_id)
        .bind(&aggregate.review_report_artifact_id)
        .bind(aggregate.authorization_version)
        .bind(&aggregate.execution_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::VersionConflict)?;
        let request_id: String = request.try_get("id")?;
        if request.try_get::<i64, _>("version")? != input.expected_request_version {
            return Err(DbError::VersionConflict);
        }
        ensure(
            input.findings.iter().all(|finding| {
                finding.request_id == request_id
                    && finding.task_id == aggregate.task_id
                    && finding.report_artifact_id == aggregate.review_report_artifact_id
                    && finding.execution_id == aggregate.execution_id
                    && finding.authorization_version == aggregate.authorization_version
            }),
            "finding dispositions must bind the exact triage request",
        )?;
        if aggregate.disposition == ReviewFindingDisposition::Recheck
            || input
                .findings
                .iter()
                .any(|finding| finding.disposition == ReviewFindingDisposition::Recheck)
        {
            let consumed: i64 = sqlx::query_scalar(
                "SELECT (SELECT COUNT(*) FROM task_rework_receipt WHERE task_id = ?)
                      + (SELECT COUNT(*) FROM review_rework_round WHERE task_id = ?)
                      + (SELECT COUNT(*) FROM task_review_triage triage WHERE triage.task_id = ?
                           AND (triage.disposition = 'recheck' OR EXISTS (
                               SELECT 1 FROM review_finding_triage finding_triage
                               WHERE finding_triage.report_artifact_id = triage.review_report_artifact_id
                                 AND finding_triage.disposition = 'recheck'
                           )))",
            )
            .bind(&aggregate.task_id)
            .bind(&aggregate.task_id)
            .bind(&aggregate.task_id)
            .fetch_one(&mut *tx)
            .await?;
            let max_rounds: i64 = sqlx::query_scalar(
                "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id = ?",
            )
            .bind(&aggregate.task_id)
            .fetch_one(&mut *tx)
            .await?;
            ensure(
                consumed < max_rounds,
                "review.rework.round_budget_exhausted",
            )?;
        }
        // The V185 guard revalidates current authority, pause and revocation.
        sqlx::query(
            "INSERT INTO task_review_triage (
                 review_report_artifact_id, task_id, report_digest, execution_id,
                 authorization_version, disposition, rationale, instructions,
                 decision_digest, review_work_unit_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&aggregate.review_report_artifact_id)
        .bind(&aggregate.task_id)
        .bind(&aggregate.report_digest)
        .bind(&aggregate.execution_id)
        .bind(aggregate.authorization_version)
        .bind(aggregate.disposition.to_string())
        .bind(&aggregate.rationale)
        .bind(&aggregate.instructions)
        .bind(&aggregate.decision_digest)
        .bind(aggregate.review_work_unit_id.as_deref())
        .bind(&aggregate.created_at)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        for finding in &input.findings {
            sqlx::query(
                "INSERT INTO review_finding_triage (
                     finding_id, request_id, task_id, report_artifact_id, execution_id,
                     authorization_version, disposition, rationale, instructions, created_at
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(&finding.finding_id)
            .bind(&finding.request_id)
            .bind(&finding.task_id)
            .bind(&finding.report_artifact_id)
            .bind(&finding.execution_id)
            .bind(finding.authorization_version)
            .bind(finding.disposition.to_string())
            .bind(&finding.rationale)
            .bind(&finding.instructions)
            .bind(&finding.created_at)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }
        if let Some(corrections) = input.corrections.as_ref() {
            ensure(
                corrections.round.task_id == aggregate.task_id
                    && corrections.round.report_artifact_id == aggregate.review_report_artifact_id
                    && corrections.round.request_id.as_deref() == Some(request_id.as_str()),
                "corrections must belong to the exact triage request",
            )?;
            apply_corrections_in_tx(self, &mut tx, corrections).await?;
        }
        if let Some(recheck) = input.recheck {
            ensure(
                recheck.checkpoint.task_id == aggregate.task_id,
                "recheck checkpoint belongs to another Task",
            )?;
            upsert_checkpoint_in_tx(self, &mut tx, recheck).await?;
        }
        let triaged = sqlx::query(
            "UPDATE task_coordination_request
             SET state = 'triaged', version = version + 1, updated_at = ?, completed_at = ?
             WHERE id = ? AND version = ? AND state IN ('running', 'uncertain')",
        )
        .bind(&aggregate.created_at)
        .bind(&aggregate.created_at)
        .bind(&request_id)
        .bind(input.expected_request_version)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if triaged.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }
        append_events(self, &mut tx, &input.events).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn apply_review_corrections(
        &self,
        input: ApplyReviewCorrections,
    ) -> Result<ReviewReworkRound> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = round_for_report(&mut tx, &input.round.report_artifact_id).await? {
            return if existing.task_id == input.round.task_id
                && existing.idempotency_key == input.round.idempotency_key
            {
                Ok(existing)
            } else {
                Err(DbError::IdempotencyConflict)
            };
        }
        let record = apply_corrections_in_tx(self, &mut tx, &input).await?;
        tx.commit().await?;
        Ok(record)
    }

    async fn count_review_rework_rounds(&self, task_id: &str) -> Result<i64> {
        Ok(
            sqlx::query_scalar(
                "SELECT (SELECT COUNT(*) FROM task_rework_receipt WHERE task_id = ?)
                      + (SELECT COUNT(*) FROM review_rework_round WHERE task_id = ?)
                      + (SELECT COUNT(*) FROM task_review_triage triage WHERE triage.task_id = ?
                           AND (triage.disposition = 'recheck' OR EXISTS (
                               SELECT 1 FROM review_finding_triage finding_triage
                               WHERE finding_triage.report_artifact_id = triage.review_report_artifact_id
                                 AND finding_triage.disposition = 'recheck'
                           )))",
            )
                .bind(task_id)
                .bind(task_id)
                .bind(task_id)
                .fetch_one(&self.pool)
                .await?,
        )
    }
}
