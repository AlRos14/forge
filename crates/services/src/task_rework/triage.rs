use super::*;
use crate::coordinator_outputs::MicroFixOutput;
use db::{
    ReviewCheckpointRepo, ReviewLedgerRepo, ReviewRangeReportRepo, TaskCoordinationRequestState,
};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewFindingsDecision {
    pub authorization_version: i64,
    pub review_report_artifact_id: String,
    pub report_digest: String,
    pub disposition: ReviewFindingsDisposition,
    pub rationale: String,
    pub instructions: String,
    #[serde(default)]
    pub micro_fix: Option<MicroFixOutput>,
}

use api_types::TaskReviewDisposition as ReviewFindingsDisposition;

/// `defer` applies only to non-blocking findings of a commit-range review.
fn defer_not_permitted() -> ServiceError {
    ServiceError::invalid_operation("defer is only valid for commit-range review findings")
}

impl TaskReworkService {
    pub async fn list_review_triage(
        &self,
        task_id: &str,
    ) -> Result<Vec<api_types::TaskReviewTriageResponse>> {
        let rows=sqlx::query("SELECT * FROM task_review_triage WHERE task_id=? ORDER BY created_at DESC,review_report_artifact_id DESC LIMIT 30")
            .bind(task_id).fetch_all(self.db.pool()).await?;
        rows.into_iter()
            .map(|row| {
                Ok(api_types::TaskReviewTriageResponse {
                    review_report_artifact_id: row.try_get("review_report_artifact_id")?,
                    execution_id: row.try_get("execution_id")?,
                    disposition: match row.try_get::<String, _>("disposition")?.as_str() {
                        "correct" => ReviewFindingsDisposition::Correct,
                        "recheck" => ReviewFindingsDisposition::Recheck,
                        "human" => ReviewFindingsDisposition::Human,
                        "defer" => ReviewFindingsDisposition::Defer,
                        _ => {
                            return Err(ServiceError::invalid_operation(
                                "Invalid persisted review disposition",
                            ))
                        }
                    },
                    rationale: row.try_get("rationale")?,
                    instructions: row.try_get("instructions")?,
                    review_work_unit_id: row.try_get("review_work_unit_id")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }
    /// Return the only report context admitted by a typed ReviewFindings
    /// request. The source view and stored request refs must still agree with
    /// the pending intent and latest review-line revision.
    pub(crate) async fn exact_review_findings_context(
        &self,
        request: &db::TaskCoordinationRequest,
    ) -> Result<serde_json::Value> {
        if request.kind != db::TaskCoordinationRequestKind::ReviewFindings {
            return Err(ServiceError::invalid_operation(
                "review context requires a ReviewFindings request",
            ));
        }
        let report_id = request
            .review_report_artifact_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("ReviewFindings request has no report source")
            })?;
        let report_digest = request.review_report_digest.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("ReviewFindings request has no report digest")
        })?;
        let review_producer_execution_id = request
            .review_producer_execution_id
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "ReviewFindings request has no report producer Execution",
                )
            })?;
        let is_range: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM review_range_report
             WHERE report_artifact_id=? AND task_id=?)",
        )
        .bind(report_id)
        .bind(&request.task_id)
        .fetch_one(self.db.pool())
        .await?;
        if is_range {
            return self
                .exact_range_review_findings_context(
                    request,
                    report_id,
                    report_digest,
                    review_producer_execution_id,
                )
                .await;
        }
        let row = sqlx::query(
            "SELECT report.id,report.digest,report.content,
                    intent.reviewed_work_unit_id,intent.reviewed_work_unit_version,
                    review.work_unit_id AS review_work_unit_id,review_unit.scope AS review_scope,
                    reviewed.scope AS original_scope,subject.result_execution_id,
                    subject.result_artifact_id,subject.result_commit_sha,
                    subject.result_workspace_id,subject.workspace_snapshot_digest,
                    result.summary AS result_summary,workspace.worktree_path AS result_path,
                    delivery.digest AS delivery_digest,delivery.content AS delivery_content,
                    delivery.content_ref AS delivery_content_ref
             FROM task_review_coordination_source_current source
             JOIN artifact report ON report.id=source.report_id
              AND report.task_id=source.task_id AND report.digest=source.report_digest
             JOIN artifact_execution_producer producer
              ON producer.artifact_id=report.id AND producer.execution_id=source.producer_execution_id
             JOIN execution review ON review.id=producer.execution_id
              AND review.task_id=report.task_id AND review.status='completed'
             JOIN work_unit review_unit ON review_unit.id=review.work_unit_id
              AND review_unit.task_id=report.task_id
             JOIN task_rework_automation_intent intent
              ON intent.task_id=report.task_id AND intent.review_report_artifact_id=report.id
             JOIN work_unit_review_subject subject
              ON subject.review_work_unit_id=review.work_unit_id
               AND subject.task_id=report.task_id
             JOIN work_unit reviewed ON reviewed.id=subject.reviewed_work_unit_id
              AND reviewed.task_id=report.task_id
             JOIN execution result ON result.id=subject.result_execution_id
              AND result.task_id=report.task_id
             LEFT JOIN workspace ON workspace.id=subject.result_workspace_id
             LEFT JOIN artifact delivery ON delivery.id=subject.result_artifact_id
             JOIN task_coordination_request request
              ON request.id=? AND request.task_id=report.task_id
               AND request.kind='review_findings'
               AND request.review_report_artifact_id=report.id
               AND request.review_report_digest=report.digest
               AND request.review_producer_execution_id=producer.execution_id
               AND request.authorization_version=source.authorization_version
               AND request.state IN ('requested','running','uncertain')
             WHERE report.id=? AND report.task_id=? AND report.digest=?
               AND producer.execution_id=? AND intent.state='pending'",
        )
        .bind(&request.id)
        .bind(report_id)
        .bind(&request.task_id)
        .bind(report_digest)
        .bind(review_producer_execution_id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation(
                "ReviewFindings source is no longer current or does not match its request",
            )
        })?;
        let content: String = row.try_get("content")?;
        if content.len() > 64 * 1024 {
            return Err(ServiceError::invalid_operation(
                "Review report exceeds the coordinator context limit; explicit Human intervention is required",
            ));
        }
        let review_scope: String = row.try_get("review_scope")?;
        if review_scope.len() > 64 * 1024 {
            return Err(ServiceError::invalid_operation(
                "Review WorkUnit scope exceeds the coordinator context limit; explicit Human intervention is required",
            ));
        }
        Ok(serde_json::json!({
            "report_artifact_id": row.try_get::<String, _>("id")?,
            "report_digest": row.try_get::<String, _>("digest")?,
            "report": content,
            "review_work_unit_id": row.try_get::<String, _>("review_work_unit_id")?,
            "review_scope": review_scope,
            "reviewed_work_unit_id": row.try_get::<String, _>("reviewed_work_unit_id")?,
            "reviewed_work_unit_version": row.try_get::<i64, _>("reviewed_work_unit_version")?,
            "reviewed_result": {
                "original_scope": row.try_get::<String, _>("original_scope")?,
                "execution_id": row.try_get::<String, _>("result_execution_id")?,
                "artifact_id": row.try_get::<Option<String>, _>("result_artifact_id")?,
                "commit_sha": row.try_get::<Option<String>, _>("result_commit_sha")?,
                "workspace_id": row.try_get::<Option<String>, _>("result_workspace_id")?,
                "workspace_snapshot_digest": row.try_get::<Option<String>, _>("workspace_snapshot_digest")?,
                "workspace_path": row.try_get::<Option<String>, _>("result_path")?,
                "summary": row.try_get::<Option<String>, _>("result_summary")?,
                "delivery_artifact": {
                    "id": row.try_get::<Option<String>, _>("result_artifact_id")?,
                    "digest": row.try_get::<Option<String>, _>("delivery_digest")?,
                    "content": row.try_get::<Option<String>, _>("delivery_content")?,
                    "content_ref": row.try_get::<Option<String>, _>("delivery_content_ref")?
                }
            }
        }))
    }

    async fn exact_range_review_findings_context(
        &self,
        request: &db::TaskCoordinationRequest,
        report_id: &str,
        report_digest: &str,
        producer_execution_id: &str,
    ) -> Result<serde_json::Value> {
        let mut source_current: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_review_coordination_source_current source
                JOIN task_coordination_request request
                  ON request.id=? AND request.task_id=source.task_id
                 AND request.kind='review_findings'
                 AND request.review_report_artifact_id=source.report_id
                 AND request.review_report_digest=source.report_digest
                 AND request.review_producer_execution_id=source.producer_execution_id
                 AND request.authorization_version=source.authorization_version
                 AND request.state IN ('requested','running','uncertain')
                WHERE source.report_id=? AND source.task_id=?
                  AND source.report_digest=? AND source.producer_execution_id=?
            )",
        )
        .bind(&request.id)
        .bind(report_id)
        .bind(&request.task_id)
        .bind(report_digest)
        .bind(producer_execution_id)
        .fetch_one(self.db.pool())
        .await?;
        if !source_current && request.state == TaskCoordinationRequestState::Triaged {
            source_current = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM task_coordination_request request
                    JOIN task_review_triage triage
                      ON triage.task_id=request.task_id
                     AND triage.review_report_artifact_id=request.review_report_artifact_id
                    JOIN review_range_report rr
                      ON rr.report_artifact_id=request.review_report_artifact_id
                     AND rr.task_id=request.task_id
                    JOIN artifact report
                      ON report.id=rr.report_artifact_id AND report.task_id=rr.task_id
                    JOIN artifact_execution_producer producer
                      ON producer.artifact_id=report.id AND producer.execution_id=rr.execution_id
                    JOIN task_coordination coordination
                      ON coordination.task_id=request.task_id
                    JOIN task_lifecycle lifecycle
                      ON lifecycle.task_id=request.task_id
                     AND lifecycle.state NOT IN ('done','cancelled','merging')
                    JOIN task_dispatch dispatch
                      ON dispatch.task_id=request.task_id AND dispatch.state='running'
                    WHERE request.id=? AND request.task_id=? AND request.kind='review_findings'
                      AND request.state='triaged'
                      AND request.review_report_artifact_id=?
                      AND request.review_report_digest=?
                      AND request.review_producer_execution_id=producer.execution_id
                      AND request.authorization_version=?
                      AND triage.execution_id=request.execution_id
                      AND triage.authorization_version=request.authorization_version
                      AND triage.report_digest=report.digest
                      AND triage.disposition='human'
                      AND rr.report_digest=report.digest
                      AND coordination.authorization_version=request.authorization_version
                      AND coordination.mode!='manual' AND coordination.revoked_at IS NULL
                      AND coordination.team_stale=0
                      AND COALESCE(json_extract(coordination.envelope_json,'$.orchestrator_reviews_findings'),0)=1
                      AND NOT EXISTS(
                          SELECT 1 FROM review_checkpoint newer
                          JOIN review_checkpoint current ON current.id=rr.checkpoint_id
                          WHERE newer.range_id=current.range_id
                            AND newer.attempt>current.attempt AND newer.state='reviewed'
                      )
                )",
            )
            .bind(&request.id)
            .bind(&request.task_id)
            .bind(report_id)
            .bind(report_digest)
            .bind(request.authorization_version)
            .fetch_one(self.db.pool())
            .await?;
        }
        if !source_current {
            return Err(ServiceError::invalid_operation(
                "range ReviewFindings source is no longer current or does not match its request",
            ));
        }
        let report = ReviewRangeReportRepo::get_review_range_report(&*self.db, report_id)
            .await?
            .filter(|report| {
                report.task_id == request.task_id && report.report_digest == report_digest
            })
            .ok_or_else(|| ServiceError::not_found("ReviewRangeReport", report_id.to_owned()))?;
        let artifact = CollaborationRepo::get_artifact(&*self.db, report_id)
            .await?
            .filter(|artifact| {
                artifact.task_id == request.task_id
                    && artifact.digest.as_deref() == Some(report_digest)
            })
            .ok_or_else(|| ServiceError::not_found("ReviewReport", report_id.to_owned()))?;
        let artifact_content = artifact
            .content
            .as_deref()
            .ok_or_else(|| ServiceError::invalid_operation("range ReviewReport is not inline"))?;
        if artifact_content.len() > 64 * 1024 {
            return Err(ServiceError::invalid_operation(
                "range ReviewReport exceeds the coordinator context limit",
            ));
        }
        let report_content: serde_json::Value = serde_json::from_str(artifact_content)
            .map_err(|_| ServiceError::invalid_operation("range ReviewReport JSON is invalid"))?;
        let range = ReviewCheckpointRepo::get_review_range(&*self.db, &report.range_id)
            .await?
            .filter(|range| range.task_id == request.task_id && range.repo_id == report.repo_id)
            .ok_or_else(|| ServiceError::invalid_operation("ReviewRange subject is stale"))?;
        let work_unit_range = ReviewCheckpointRepo::get_work_unit_review_range(
            &*self.db,
            &report.review_work_unit_id,
        )
        .await?
        .filter(|subject| {
            subject.task_id == request.task_id
                && subject.range_id == report.range_id
                && subject.repo_id == report.repo_id
        })
        .ok_or_else(|| ServiceError::invalid_operation("reviewer WorkUnit range is stale"))?;
        let coverage_rows = ReviewLedgerRepo::load_review_coverage_rows(
            &*self.db,
            &request.task_id,
            &report.repo_id,
        )
        .await?;
        let mut finding_ids = coverage_rows
            .findings
            .iter()
            .filter(|finding| finding.report_artifact_id == report_id)
            .map(|finding| finding.id.clone())
            .collect::<std::collections::BTreeSet<_>>();
        for input in &work_unit_range.input_findings {
            let resolved_by_this_report = coverage_rows.resolutions.iter().any(|resolution| {
                resolution.finding_id == input.finding_id
                    && resolution.report_artifact_id.as_deref() == Some(report_id)
            });
            if !resolved_by_this_report {
                finding_ids.insert(input.finding_id.clone());
            }
        }
        if finding_ids.len() > 256 {
            return Err(ServiceError::invalid_operation(
                "range review findings exceed the coordinator context limit",
            ));
        }
        let mut findings = Vec::with_capacity(finding_ids.len());
        for finding_id in finding_ids {
            let finding = coverage_rows
                .findings
                .iter()
                .find(|finding| finding.id == finding_id)
                .ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "range review input finding {finding_id} is missing"
                    ))
                })?;
            let locations = ReviewLedgerRepo::list_review_finding_locations(&*self.db, &finding_id)
                .await?
                .into_iter()
                .map(|location| {
                    serde_json::json!({
                        "path": location.path,
                        "line_start": location.line_start,
                        "line_end": location.line_end,
                        "attribution": {
                            "kind": location.attribution.kind.to_string(),
                            "work_unit_id": location.attribution.work_unit_id,
                            "execution_id": location.attribution.execution_id,
                            "actor": location.attribution.actor,
                            "commit_sha": location.attribution.commit_sha,
                            "integration_id": location.attribution.integration_id,
                            "line_count": location.attribution.line_count,
                        }
                    })
                })
                .collect::<Vec<_>>();
            findings.push(serde_json::json!({
                "finding_id": finding.id,
                "ordinal": finding.ordinal,
                "severity": finding.severity.to_string(),
                "title": finding.title,
                "evidence": finding.evidence,
                "invariant": finding.invariant,
                "locations": locations,
            }));
        }
        let repository_set = db::TaskRepositorySetRepo::get_current(&*self.db, &request.task_id)
            .await?
            .ok_or_else(|| ServiceError::invalid_operation("Task has no current repository set"))?;
        let coverage_service = crate::review_range::ReviewCoverageService::new(
            Arc::clone(&self.db),
            crate::task_service::workspace::default_workspace_root(),
        );
        let mut coverage_by_repo = Vec::new();
        for repository in repository_set
            .repositories
            .into_iter()
            .filter(|repository| {
                matches!(
                    repository.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
            })
        {
            let rows = if repository.repo_id == report.repo_id {
                coverage_rows.clone()
            } else {
                ReviewLedgerRepo::load_review_coverage_rows(
                    &*self.db,
                    &request.task_id,
                    &repository.repo_id,
                )
                .await?
            };
            let candidate = rows
                .integrations
                .iter()
                .filter(|integration| {
                    integration.outcome == db::WorkUnitRepositoryIntegrationOutcome::Success
                })
                .max_by(|left, right| {
                    (&left.started_at, &left.id).cmp(&(&right.started_at, &right.id))
                })
                .and_then(|integration| integration.target_after_sha.as_deref());
            let coverage = coverage_service
                .compute(&request.task_id, &repository.repo_id, candidate)
                .await?;
            coverage_by_repo.push(serde_json::json!({
                "repo_id": repository.repo_id,
                "candidate_commit": candidate,
                "status": coverage.status.as_str(),
                "frontier_commit": coverage.frontier_commit,
                "open_findings": {
                    "p0": coverage.open_findings.p0,
                    "p1": coverage.open_findings.p1,
                    "p2": coverage.open_findings.p2,
                    "p3": coverage.open_findings.p3,
                },
            }));
        }
        let context = serde_json::json!({
            "kind": "commit_range",
            "report_artifact_id": report_id,
            "report_digest": report_digest,
            "report": report_content,
            "review_work_unit_id": report.review_work_unit_id,
            "review_producer_execution_id": producer_execution_id,
            "range": {
                "id": range.id,
                "repo_id": range.repo_id,
                "base_commit": range.base_commit,
                "head_commit": range.head_commit,
                "base_tree": range.base_tree,
                "head_tree": range.head_tree,
                "range_digest": range.range_digest,
            },
            "checkpoint_id": report.checkpoint_id,
            "findings": findings,
            "coverage_by_repo": coverage_by_repo
        });
        let context_bytes = serde_json::to_vec(&context)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        if context_bytes.len() > 64 * 1024 {
            return Err(ServiceError::invalid_operation(
                "range ReviewFindings context exceeds 64 KiB",
            ));
        }
        Ok(context)
    }

    pub(crate) async fn record_review_triage(
        &self,
        execution_id: &str,
        request_id: &str,
        input: ReviewFindingsDecision,
    ) -> Result<()> {
        if matches!(input.disposition, ReviewFindingsDisposition::Defer) {
            return Err(defer_not_permitted());
        }
        if input.rationale.trim().is_empty()
            || input.rationale.len() > 8192
            || input.instructions.trim().is_empty()
            || input.instructions.len() > 16384
        {
            return Err(ServiceError::invalid_operation(
                "Review triage requires bounded rationale and actionable instructions",
            ));
        }
        let decision_digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&input)
                .map_err(|e| ServiceError::invalid_operation(e.to_string()))?,
        ));
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT decision_digest FROM task_review_triage WHERE review_report_artifact_id=?",
        )
        .bind(&input.review_report_artifact_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing != decision_digest {
                return Err(DbError::IdempotencyConflict.into());
            }
            let exact_request: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM task_coordination_request request
                    JOIN task_review_triage triage
                      ON triage.review_report_artifact_id=request.review_report_artifact_id
                     AND triage.task_id=request.task_id
                    WHERE request.id=? AND request.kind='review_findings'
                      AND request.state='triaged' AND request.execution_id=?
                      AND request.review_report_artifact_id=?
                      AND request.review_report_digest=?
                      AND request.authorization_version=?
                      AND triage.execution_id=request.execution_id
                      AND triage.report_digest=request.review_report_digest
                )",
            )
            .bind(request_id)
            .bind(execution_id)
            .bind(&input.review_report_artifact_id)
            .bind(&input.report_digest)
            .bind(input.authorization_version)
            .fetch_one(&mut *tx)
            .await?;
            if !exact_request {
                return Err(DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return Ok(());
        }
        // Bind the exact running wake, current coordinator, authority snapshot,
        // report and current team under the SQLite writer. A report cannot be
        // waived, rewritten, or triaged on behalf of another coordinator.
        let row = sqlx::query("SELECT c.task_id,c.authorization_version,c.max_turns,c.turn_count,e.actor_kind,e.actor_id,
                report.digest,producer.execution_id AS reviewer_execution_id,review.role AS review_role,
                review.actor_kind AS reviewer_kind,review.actor_id AS reviewer_actor_id,
                s.review_work_unit_id,s.reviewed_work_unit_id
            FROM task_coordination c JOIN task task ON task.id=c.task_id
            JOIN project project ON project.id=task.project_id
            JOIN execution e ON e.id=? AND e.task_id=c.task_id
            JOIN task_coordination_request request ON request.id=? AND request.task_id=c.task_id
              AND request.kind='review_findings' AND request.state IN ('running','uncertain')
              AND request.execution_id=e.id AND request.authorization_version=c.authorization_version
            JOIN agent_current agent ON agent.id=e.actor_id AND e.actor_kind='agent' AND agent.owner_id=project.owner_id
            JOIN project_member owner_member ON owner_member.project_id=project.id AND owner_member.user_id=agent.owner_id AND owner_member.role IN ('owner','admin','member')
            JOIN execution_task_role_authority a ON a.execution_id=e.id
            JOIN task_role role ON role.id=a.task_role_id
            JOIN task_role_capability_configuration config ON config.task_role_id=role.id
            JOIN role_membership membership ON membership.id=a.membership_id
            JOIN orchestrator_wake_execution attempt ON attempt.execution_id=e.id
            JOIN orchestrator_wake wake ON wake.id=attempt.wake_id
            JOIN artifact report ON report.id=? AND report.task_id=c.task_id
            JOIN task_rework_automation_intent intent ON intent.task_id=c.task_id AND intent.review_report_artifact_id=report.id AND intent.state='pending'
            JOIN artifact_execution_producer producer ON producer.artifact_id=report.id
            JOIN execution review ON review.id=producer.execution_id
            JOIN work_unit_review_subject s ON s.review_work_unit_id=review.work_unit_id
            JOIN task_lifecycle lifecycle ON lifecycle.task_id=c.task_id
            JOIN task_dispatch dispatch ON dispatch.task_id=c.task_id
            WHERE c.mode!='manual' AND c.revoked_at IS NULL AND c.team_stale=0
              AND request.wake_id=wake.id
              AND EXISTS(SELECT 1 FROM task_review_coordination_request_current current
                         WHERE current.request_id=request.id)
              AND (json_type(wake.task_role_policy_json,'$.max_actions_per_execution') IS NULL
                   OR (json_type(wake.task_role_policy_json,'$.max_actions_per_execution')='integer'
                       AND json_extract(wake.task_role_policy_json,'$.max_actions_per_execution')>=1))
              AND (json_type(wake.task_role_policy_json,'$.allowed_actions') IS NULL
                   OR json_type(wake.task_role_policy_json,'$.allowed_actions')='null'
                   OR (json_type(wake.task_role_policy_json,'$.allowed_actions')='array'
                       AND EXISTS(SELECT 1 FROM json_each(wake.task_role_policy_json,'$.allowed_actions')
                                  WHERE value='review_findings')))
              AND COALESCE(json_extract(c.envelope_json,'$.orchestrator_reviews_findings'),0)=1
              AND c.coordinator_task_role_id=a.task_role_id AND c.coordinator_actor_kind=e.actor_kind AND c.coordinator_actor_id=e.actor_id
              AND task.deleted_at IS NULL AND e.status IN ('running','completed') AND e.work_unit_id IS NULL
              AND wake.current_attempt=attempt.attempt_number AND attempt.state IN ('running','completed')
              AND wake.state IN ('leased','running','uncertain')
              AND (e.status='completed' OR wake.lease_until IS NULL OR julianday(wake.lease_until)>julianday('now'))
              AND wake.task_role_version=role.version AND wake.task_role_policy_json=role.policy_json
              AND a.task_role_version=role.version AND a.configuration_json=config.configuration_json
              AND membership.status='active' AND membership.task_role_id=role.id AND membership.actor_kind=e.actor_kind AND membership.actor_id=e.actor_id
              AND EXISTS(SELECT 1 FROM json_each(a.capabilities_json) WHERE value='orchestration')
              AND lifecycle.state NOT IN ('cancelled','done','merging') AND dispatch.state NOT IN ('paused','cancelled')
              AND report.kind='review_report' AND report.digest=? AND json_extract(report.content,'$.verdict')='request_changes'
              AND review.status='completed' AND c.authorization_version=?
              AND request.review_report_artifact_id=report.id
              AND request.review_report_digest=report.digest
              AND request.review_producer_execution_id=producer.execution_id
              AND NOT EXISTS(SELECT 1 FROM work_unit_review_line_revision rev JOIN work_unit_review_line line ON line.id=rev.line_id
                             WHERE rev.review_report_artifact_id=report.id AND line.latest_report_artifact_id IS NOT report.id)")
            .bind(execution_id)
            .bind(request_id)
            .bind(&input.review_report_artifact_id)
            .bind(&input.report_digest)
            .bind(input.authorization_version)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(row) = row else {
            let dispatch_paused: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM task_coordination_request request
                    JOIN task_dispatch dispatch ON dispatch.task_id=request.task_id
                    WHERE request.id=? AND dispatch.state='paused'
                    UNION ALL
                    SELECT 1 FROM project_plan_batch_coordination_request scope
                    JOIN project_plan_batch batch ON batch.id=scope.batch_id
                    WHERE scope.request_id=? AND batch.state='paused'
                )",
            )
            .bind(request_id)
            .bind(request_id)
            .fetch_one(&mut *tx)
            .await?;
            if dispatch_paused {
                return Err(crate::ServiceError::Coordination {
                    code: "coordination.paused",
                    message: "ReviewFindings triage waits until the same Task or Project batch is resumed".to_owned(),
                });
            }
            return Err(ServiceError::invalid_operation(
                "The exact report, current coordinator, wake, team or Task authority is no longer valid",
            ));
        };
        let task_id: String = row.try_get("task_id")?;
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let now = now_rfc3339();
        let actor = actor_ref_from_parts(
            &row.try_get::<String, _>("actor_kind")?,
            row.try_get::<String, _>("actor_id")?,
        )?;
        let created_by = match &actor {
            ActorRef::Agent(id) => db::ActorRef::Agent(id.clone()),
            ActorRef::Human(id) => db::ActorRef::Human(id.clone()),
        };
        let review_id = if matches!(input.disposition, ReviewFindingsDisposition::Recheck) {
            let consumed:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM task_rework_receipt WHERE task_id=?)+(SELECT count(*) FROM review_rework_round WHERE task_id=?)+(SELECT count(*) FROM task_review_triage triage WHERE triage.task_id=? AND (triage.disposition='recheck' OR EXISTS (SELECT 1 FROM review_finding_triage finding_triage WHERE finding_triage.report_artifact_id=triage.review_report_artifact_id AND finding_triage.disposition='recheck')))")
                .bind(&task_id).bind(&task_id).bind(&task_id).fetch_one(&mut *tx).await?;
            let max_rounds: i64 = sqlx::query_scalar(
                "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id=?",
            )
            .bind(&task_id)
            .fetch_one(&mut *tx)
            .await?;
            if consumed >= max_rounds {
                return Err(ServiceError::conflict("Recheck and correction round budget exhausted; a Human may authorize extra rounds"));
            }
            let role: String = row.try_get("review_role")?;
            let reviewer = actor_ref_from_parts(
                &row.try_get::<String, _>("reviewer_kind")?,
                row.try_get::<String, _>("reviewer_actor_id")?,
            )?;
            let eligible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_role r JOIN role_membership m ON m.task_role_id=r.id JOIN task_role_capability_configuration config ON config.task_role_id=r.id WHERE r.task_id=? AND r.role=? AND m.actor_kind=? AND m.actor_id=? AND m.status='active' AND EXISTS(SELECT 1 FROM json_each(config.configuration_json,'$.capabilities') WHERE value='formal_review'))")
                .bind(&task_id).bind(&role).bind(match &reviewer {ActorRef::Agent(_)=>"agent",ActorRef::Human(_)=>"human"})
                .bind(match &reviewer {ActorRef::Agent(id)|ActorRef::Human(id)=>id}).fetch_one(&mut *tx).await?;
            if !eligible {
                return Err(ServiceError::invalid_operation(
                    "The independent review destination is no longer eligible",
                ));
            }
            let id = new_uuid_v4();
            insert_work_unit_in_tx(&mut tx,&task_id,&id,"Reassess disputed review findings",&format!("Independently reassess the exact reviewed result. The coordinator disputes or needs clarification of these findings; do not accept the coordinator's conclusion without examining the result.\n\nRationale: {}\nInstructions: {}\nOriginal exact ReviewReport: {}",input.rationale,input.instructions,input.review_report_artifact_id),&role,Some(&reviewer),false,Some(("artifact",&input.review_report_artifact_id,None)),&created_by,&now).await?;
            sqlx::query("INSERT INTO work_unit_review_subject(review_work_unit_id,task_id,reviewed_work_unit_id,reviewed_work_unit_version,result_execution_id,result_artifact_id,result_commit_sha,result_workspace_id,workspace_snapshot_digest,created_at)
                SELECT ?,task_id,reviewed_work_unit_id,reviewed_work_unit_version,result_execution_id,result_artifact_id,result_commit_sha,result_workspace_id,workspace_snapshot_digest,? FROM work_unit_review_subject WHERE review_work_unit_id=?")
                .bind(&id).bind(&now).bind(row.try_get::<String,_>("review_work_unit_id")?).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO work_unit_artifact_input(work_unit_id,task_id,artifact_id,artifact_digest,producer_execution_id,created_at) VALUES(?,?,?,?,?,?)")
                .bind(&id).bind(&task_id).bind(&input.review_report_artifact_id).bind(&input.report_digest).bind(row.try_get::<String,_>("reviewer_execution_id")?).bind(&now).execute(&mut *tx).await?;
            Some(id)
        } else {
            None
        };
        let disposition = match input.disposition {
            ReviewFindingsDisposition::Correct => "correct",
            ReviewFindingsDisposition::Recheck => "recheck",
            ReviewFindingsDisposition::Human => "human",
            ReviewFindingsDisposition::Defer => return Err(defer_not_permitted()),
        };
        sqlx::query("INSERT INTO task_review_triage(review_report_artifact_id,task_id,report_digest,execution_id,authorization_version,disposition,rationale,instructions,decision_digest,review_work_unit_id,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&input.review_report_artifact_id).bind(&task_id).bind(&input.report_digest).bind(execution_id).bind(authorization_version).bind(disposition).bind(&input.rationale).bind(&input.instructions).bind(&decision_digest).bind(review_id.as_deref()).bind(&now).execute(&mut *tx).await?;
        let request_changed = sqlx::query(
            "UPDATE task_coordination_request
             SET state='triaged',version=version+1,updated_at=?,completed_at=?
             WHERE id=? AND task_id=? AND kind='review_findings'
               AND state IN ('running','uncertain') AND execution_id=?
               AND authorization_version=? AND review_report_artifact_id=?
               AND review_report_digest=? AND review_producer_execution_id=?",
        )
        .bind(&now)
        .bind(&now)
        .bind(request_id)
        .bind(&task_id)
        .bind(execution_id)
        .bind(authorization_version)
        .bind(&input.review_report_artifact_id)
        .bind(&input.report_digest)
        .bind(row.try_get::<String, _>("reviewer_execution_id")?)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if request_changed != 1 {
            return Err(ServiceError::conflict(
                "ReviewFindings request changed before the triage receipt was committed",
            ));
        }
        let work_event = if let Some(id) = review_id.as_ref() {
            Some(DomainEventRepo::append_event_in_tx(&*self.db, &mut tx, &CreateDomainEvent {
                id:new_uuid_v4(),event_type:"work_unit.created".to_owned(),entity_type:"work_unit".to_owned(),entity_id:id.clone(),
                actor_type:created_by.kind().to_string(),actor_id:Some(created_by.id().to_owned()),scope_type:"task".to_owned(),scope_id:task_id.clone(),correlation_id:task_id.clone(),causation_id:None,causation_depth:0,dedupe_key:Some(format!("work_unit.created:{id}")),
                payload_json:serde_json::json!({"task_id":task_id,"work_unit_id":id,"version":1,"status":"open","review_report_artifact_id":input.review_report_artifact_id}).to_string(),created_at:now.clone(),
            }).await?)
        } else {
            None
        };
        let event=DomainEventRepo::append_event_in_tx(&*self.db,&mut tx,&CreateDomainEvent {
            id:new_uuid_v4(),event_type:"task.review_triaged".to_owned(),entity_type:"artifact".to_owned(),entity_id:input.review_report_artifact_id.clone(),
            actor_type:match &actor {ActorRef::Agent(_)=>"agent",ActorRef::Human(_)=>"human"}.to_owned(),
            actor_id:Some(match &actor {ActorRef::Agent(id)|ActorRef::Human(id)=>id.clone()}),scope_type:"task".to_owned(),scope_id:task_id.clone(),correlation_id:task_id.clone(),causation_id:None,causation_depth:0,dedupe_key:Some(format!("review-triage:{}",input.review_report_artifact_id)),
            payload_json:serde_json::json!({"task_id":task_id,"review_report_artifact_id":input.review_report_artifact_id,"disposition":disposition,"review_work_unit_id":review_id}).to_string(),created_at:now.clone(),
        }).await?;
        let request_event = DomainEventRepo::append_event_in_tx(
            &*self.db,
            &mut tx,
            &CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.coordination_request_state_changed".to_owned(),
                entity_type: "task_coordination_request".to_owned(),
                entity_id: request_id.to_owned(),
                actor_type: match &actor {
                    ActorRef::Agent(_) => "agent",
                    ActorRef::Human(_) => "human",
                }
                .to_owned(),
                actor_id: Some(match &actor {
                    ActorRef::Agent(id) | ActorRef::Human(id) => id.clone(),
                }),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: task_id.clone(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: Some(format!("review-findings-triaged:{request_id}")),
                payload_json: serde_json::json!({
                    "task_id": task_id,
                    "request_id": request_id,
                    "state": "triaged",
                    "review_report_artifact_id": input.review_report_artifact_id,
                    "disposition": disposition,
                })
                .to_string(),
                created_at: now.clone(),
            },
        )
        .await?;
        tx.commit().await?;
        if let Some(event) = work_event.as_ref() {
            crate::domain_event_service::DomainEventService::publish_committed_hint(
                &self.event_bus,
                event,
            );
        }
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.event_bus,
            &event,
        );
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.event_bus,
            &request_event,
        );
        Ok(())
    }
}
