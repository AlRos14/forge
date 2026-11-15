//! Node RT (phase 1): `validation_findings` coordination requests.
//!
//! A terminal catalog check (failed/error/stale run, or a baseline comparison
//! classified `regression`/`inconclusive`) becomes one typed coordination
//! request. It reuses the `review_findings` machinery: the request, its Project
//! batch origin, the turn-budget charge (`turn_count`) and the event are written
//! by the same repository transaction, the idempotency key is the exact source,
//! and a durable scanner recreates pending sources without a second charge.

use db::{
    ValidationCatalogRepo, ValidationCheckRequestRepo, ValidationCheckRequestState,
    ValidationCheckRequestTrigger, ValidationComparison, ValidationComparisonClass,
    ValidationComparisonRepo, ValidationRun, ValidationRunRepo, ValidationRunStatus,
};

use super::*;
use crate::coordinator_outputs::CheckPins;

impl CheckPins for CoordinationEnvelope {
    fn pins_coordinator_request(&self, check_key: &str) -> bool {
        self.validation_checks.iter().any(|pin| {
            pin.check_key == check_key && pin.trigger == db::EnvelopeCheckTrigger::OnRequest
        })
    }
}

const MAX_TAIL_CHARS: usize = 8192;

fn cut(value: &str, max: usize) -> String {
    value.chars().take(max).collect()
}

/// Digest the coordinator must echo as `run_digest`: the digest of the run's
/// immutable Evidence row.
pub(crate) async fn validation_run_source_digest(
    db: &SqliteDb,
    run_id: &str,
) -> Result<Option<(String, String)>> {
    let evidence = ValidationRunRepo::list_evidence_for_validation_run(db, run_id).await?;
    Ok(evidence
        .into_iter()
        .next()
        .map(|evidence| (evidence.id, evidence.digest)))
}

/// Exact, bounded run data (status, exit code, output tails and digest).
pub(crate) async fn validation_run_context(db: &SqliteDb, run: &ValidationRun) -> Result<Value> {
    let evidence = ValidationRunRepo::list_evidence_for_validation_run(db, &run.id)
        .await?
        .into_iter()
        .next();
    let content: Value = evidence
        .as_ref()
        .and_then(|evidence| serde_json::from_str(&evidence.content_json).ok())
        .unwrap_or(Value::Null);
    let tail = |key: &str| {
        content
            .get(key)
            .and_then(Value::as_str)
            .map(|value| cut(value, MAX_TAIL_CHARS))
    };
    let stdout_tail = tail("stdout_tail");
    let stderr_tail = tail("stderr_tail");
    let output_digest = crate::validation_service::output_digest(
        stdout_tail.as_deref().unwrap_or_default(),
        stderr_tail.as_deref().unwrap_or_default(),
    );
    Ok(json!({
        "validation_run_id": run.id,
        "status": run.status,
        "exit_code": run.exit_code,
        "check_identity": run.check_identity,
        "run_origin": run.run_origin,
        "definition_digest": run.definition_digest,
        "config_digest": run.config_digest,
        "workspace_id": run.workspace_id,
        "commit_sha": run.commit_sha,
        "workspace_snapshot_digest": run.workspace_snapshot_digest,
        "started_at": run.started_at,
        "finished_at": run.finished_at,
        "evidence_id": evidence.as_ref().map(|evidence| evidence.id.clone()),
        "evidence_digest": evidence.as_ref().map(|evidence| evidence.digest.clone()),
        "stdout_tail": stdout_tail,
        "stderr_tail": stderr_tail,
        "stdout_bytes": content.get("stdout_bytes"),
        "stderr_bytes": content.get("stderr_bytes"),
        "spawn_error": content.get("spawn_error"),
        "output_digest": output_digest,
    }))
}

pub(crate) fn validation_comparison_context(comparison: &ValidationComparison) -> Value {
    json!({
        "comparison_id": comparison.id,
        "check_request_id": comparison.check_request_id,
        "classification": comparison.classification,
        "same_output_digest": comparison.same_output_digest,
        "candidate_status": comparison.candidate_status,
        "candidate_output_digest": comparison.candidate_output_digest,
        "baseline_commit_sha": comparison.baseline_commit_sha,
        "baseline_snapshot_digest": comparison.baseline_snapshot_digest,
        "baseline_status": comparison.baseline_status,
        "baseline_exit_code": comparison.baseline_exit_code,
        "baseline_output_digest": comparison.baseline_output_digest,
    })
}

/// Runs of the exact reviewed subject, for review triage context.
pub(crate) async fn related_validation_context(
    db: &SqliteDb,
    task_id: &str,
    workspace_id: &str,
    commit_sha: &str,
    snapshot_digest: &str,
) -> Result<Value> {
    let ids: Vec<String> = sqlx::query_scalar(
        "SELECT id FROM validation_run
         WHERE task_id=? AND workspace_id=? AND commit_sha=? AND workspace_snapshot_digest=?
           AND status!='running'
         ORDER BY created_at DESC, id DESC LIMIT 8",
    )
    .bind(task_id)
    .bind(workspace_id)
    .bind(commit_sha)
    .bind(snapshot_digest)
    .fetch_all(db.pool())
    .await?;
    let mut runs = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(run) = ValidationRunRepo::get_validation_run(db, &id).await? else {
            continue;
        };
        let mut value = validation_run_context(db, &run).await?;
        let comparison: Option<ValidationComparison> = {
            let check_request_id: Option<String> = run.check_request_id.clone();
            match check_request_id {
                Some(request_id) => {
                    ValidationComparisonRepo::get_validation_comparison_by_request(db, &request_id)
                        .await?
                }
                None => None,
            }
        };
        value["comparison"] = comparison
            .as_ref()
            .map(validation_comparison_context)
            .unwrap_or(Value::Null);
        runs.push(value);
    }
    Ok(Value::Array(runs))
}

impl TaskCoordinationService {
    /// Exact Project batch binding for a typed continuation that has no
    /// producer Artifact (`validation_triage`, `human_decision_resume`).
    pub async fn project_batch_binding_for_task_origin(
        &self,
        task_id: &str,
        user_id: &str,
        expected_task_version: i64,
        expected_coordination_version: i64,
        origin: ProjectPlanBatchRequestOrigin,
    ) -> Result<Option<ProjectPlanBatchRequestBinding>> {
        let rows: Vec<(String, String, String, i64, i64, String)> = sqlx::query_as(
            "SELECT b.id,b.project_id,b.authorized_by,b.version,b.graph_version,
                    bt.coordination_snapshot_digest
             FROM project_plan_batch b
             JOIN project_plan p ON p.project_id=b.project_id
             JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=?
             JOIN task t ON t.id=bt.task_id AND t.project_id=b.project_id
              AND t.version=? AND t.deleted_at IS NULL
             JOIN task_coordination c ON c.task_id=t.id
             JOIN project_plan_batch_coordination_authorization a
               ON a.batch_id=b.id AND a.task_id=t.id
              AND a.authorization_version=c.authorization_version
             JOIN project_member m ON m.project_id=b.project_id
              AND m.user_id=b.authorized_by AND m.role IN ('owner','admin')
             JOIN project owner_project ON owner_project.id=b.project_id
             JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
              AND owner_membership.user_id=owner_project.owner_id AND owner_membership.role='owner'
             WHERE b.state='running' AND b.preparation_complete=1
              AND b.graph_version=p.version AND bt.state='started'
              AND b.authorized_by=? AND c.requested_by=b.authorized_by
              AND c.authorization_version=? AND c.mode='prepare_and_execute'
              AND c.revoked_at IS NULL AND c.team_stale=0
              AND bt.coordination_snapshot_json IS NOT NULL
              AND bt.coordination_snapshot_digest IS NOT NULL
              AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
              AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
              AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
              AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
              AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
              AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
              AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
              AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
              AND a.expected_graph_version=b.graph_version
              AND a.task_title=t.title AND a.task_description IS t.description
              AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
              AND EXISTS(SELECT 1 FROM task_lifecycle l WHERE l.task_id=t.id
                         AND l.state NOT IN ('done','cancelled','merging'))
              AND NOT EXISTS(SELECT 1 FROM task_dispatch d
                             WHERE d.task_id=t.id AND d.state='cancelled')",
        )
        .bind(task_id)
        .bind(expected_task_version)
        .bind(user_id)
        .bind(expected_coordination_version)
        .fetch_all(self.db.pool())
        .await?;
        if rows.len() > 1 {
            return Err(db::DbError::ProjectPlanBatchInactive.into());
        }
        Ok(rows.into_iter().next().map(
            |(batch_id, project_id, authorized_by, batch_version, graph_version, digest)| {
                ProjectPlanBatchRequestBinding {
                    project_id,
                    batch_id,
                    authorized_by,
                    expected_batch_version: batch_version,
                    expected_graph_version: graph_version,
                    expected_task_version,
                    expected_coordination_version,
                    coordination_snapshot_digest: digest,
                    origin,
                }
            },
        ))
    }

    /// Create (or replay) the typed triage request for one terminal check
    /// request. Returns `None` when the result does not wake the coordinator
    /// (passed, fixed, pre-existing failure, both pass), when authority is not
    /// current, or when a paused/stale batch retains the source for the scanner.
    pub async fn request_validation_findings_for_check(
        &self,
        check_request_id: &str,
    ) -> Result<Option<(TaskCoordinationRequest, bool)>> {
        let Some(check) =
            ValidationCheckRequestRepo::get_validation_check_request(&*self.db, check_request_id)
                .await?
        else {
            return Ok(None);
        };
        if check.state != ValidationCheckRequestState::Completed
            || !matches!(
                check.trigger_kind,
                ValidationCheckRequestTrigger::CoordinatorRequest
                    | ValidationCheckRequestTrigger::AfterIntegration
            )
        {
            return Ok(None);
        }
        let Some(run_id) = check.candidate_run_id.clone() else {
            return Ok(None);
        };
        let Some(run) = ValidationRunRepo::get_validation_run(&*self.db, &run_id).await? else {
            return Ok(None);
        };
        let comparison =
            ValidationComparisonRepo::get_validation_comparison_by_request(&*self.db, &check.id)
                .await?;
        let (source_kind, source_id) = if check.baseline {
            match comparison.as_ref() {
                Some(comparison)
                    if matches!(
                        comparison.classification,
                        ValidationComparisonClass::Regression
                            | ValidationComparisonClass::Inconclusive
                    ) =>
                {
                    ("validation_comparison", comparison.id.clone())
                }
                _ => return Ok(None),
            }
        } else if matches!(
            run.status,
            ValidationRunStatus::Failed | ValidationRunStatus::Error | ValidationRunStatus::Stale
        ) {
            ("validation_run", run.id.clone())
        } else {
            return Ok(None);
        };
        let Some((_, run_digest)) = validation_run_source_digest(&self.db, &run.id).await? else {
            return Ok(None);
        };
        let task_id = check.task_id.clone();
        let Some(coordination) =
            TaskCoordinationRepo::get_task_coordination(&*self.db, &task_id).await?
        else {
            return Ok(None);
        };
        if coordination.authorization_version != check.authorization_version {
            return Ok(None);
        }
        let idempotency_key = format!(
            "validation-findings:{}:{source_kind}:{source_id}",
            coordination.authorization_version
        );
        if let Some(existing) = TaskCoordinationRepo::get_task_coordination_request_by_key(
            &*self.db,
            &task_id,
            &idempotency_key,
        )
        .await?
        {
            if existing.kind != TaskCoordinationRequestKind::ValidationFindings
                || existing.source_subject_kind.as_deref() != Some(source_kind)
                || existing.source_subject_id.as_deref() != Some(source_id.as_str())
                || existing.source_subject_digest.as_deref() != Some(run_digest.as_str())
            {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            return Ok(Some((existing, false)));
        }
        if coordination.mode == TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
        {
            return Ok(None);
        }
        let Some(requested_by) = coordination.requested_by.clone() else {
            return Ok(None);
        };
        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task_id).await?;
        if lifecycle.is_none_or(|lifecycle| {
            matches!(
                lifecycle.state,
                TaskLifecycleState::Cancelled
                    | TaskLifecycleState::Done
                    | TaskLifecycleState::Merging
            )
        }) {
            return Ok(None);
        }
        let team = self.current_team(&task_id).await?;
        if self.team_digest(&team)? != coordination.team_digest {
            return Ok(None);
        }
        let coordinator = self.resolve_coordinator(&coordination, &team).await?;
        if coordinator
            .as_ref()
            .is_none_or(|value| value.actor_ref.kind() != db::ActorKind::Agent)
        {
            return Ok(None);
        }
        let batch_binding =
            if TaskCoordinationRepo::has_current_project_plan_batch_coordination_authorization(
                &*self.db, &task_id,
            )
            .await?
            {
                match self
                    .project_batch_binding_for_task_origin(
                        &task_id,
                        &requested_by,
                        task.version,
                        coordination.authorization_version,
                        ProjectPlanBatchRequestOrigin::ValidationTriage,
                    )
                    .await?
                {
                    Some(binding) => Some(binding),
                    // Paused or stale batch: the scanner retries after resume.
                    None => return Ok(None),
                }
            } else {
                None
            };
        let brief = CoordinationBrief {
            objective: format!("Triage validation {source_kind} {source_id} ({run_digest})"),
            constraints: Some(
                "Decide only correct, recheck, or human for this exact validation result. Do not rewrite its status, widen scope, or approve a Gate.".to_owned(),
            ),
            acceptance_criteria: vec![
                "Use only the exact run and comparison supplied in the request context".to_owned(),
            ],
            context: Some(
                json!({
                    "task_version": task.version,
                    "authorization_version": coordination.authorization_version,
                    "coordinator_task_role_id": coordination.coordinator_task_role_id,
                    "coordinator_actor": coordination.coordinator_actor,
                    "team_generation": coordination.team_generation,
                    "team_digest": coordination.team_digest,
                    "envelope_digest": coordination.envelope_digest,
                    "check_request_id": check.id,
                    "validation_run_id": run.id,
                    "source_subject_kind": source_kind,
                    "source_subject_id": source_id,
                    "source_subject_digest": run_digest,
                })
                .to_string(),
            ),
        };
        brief.validate()?;
        let brief_json = canonical_json(&brief)?;
        let objective_digest = sha256_hex(brief_json.as_bytes());
        let request_digest = sha256_hex(
            canonical_json(&json!({
                "kind": "validation_findings",
                "task_id": task.id,
                "task_version": task.version,
                "authorization_version": coordination.authorization_version,
                "team_generation": coordination.team_generation,
                "team_digest": coordination.team_digest,
                "envelope_digest": coordination.envelope_digest,
                "source_subject_kind": source_kind,
                "source_subject_id": source_id,
                "source_subject_digest": run_digest,
            }))?
            .as_bytes(),
        );
        let request_id = new_uuid_v4();
        let now = now_rfc3339();
        let mut event = self.event(
            "task.coordination_requested",
            "task_coordination_request",
            &request_id,
            "system",
            None,
            &task.id,
            json!({
                "task_id": task.id,
                "request_id": request_id,
                "kind": TaskCoordinationRequestKind::ValidationFindings.to_string(),
                "authorization_version": coordination.authorization_version,
                "source_subject_kind": source_kind,
                "source_subject_id": source_id,
                "source_subject_digest": run_digest,
                "objective_digest": objective_digest,
            }),
            &now,
        );
        event.dedupe_key = Some(format!(
            "validation-findings-request:{}:{source_id}",
            coordination.authorization_version
        ));
        let create_input = db::CreateTaskCoordinationRequestInput {
            id: request_id,
            task_id: task.id.clone(),
            kind: TaskCoordinationRequestKind::ValidationFindings,
            review_report_artifact_id: None,
            review_report_digest: None,
            review_producer_execution_id: None,
            source_subject_kind: Some(source_kind.to_owned()),
            source_subject_id: Some(source_id.clone()),
            source_subject_digest: Some(run_digest),
            resumes_request_id: None,
            requested_by,
            expected_task_version: task.version,
            authorization_version: coordination.authorization_version,
            mode: coordination.mode,
            brief_json,
            objective_digest,
            request_digest,
            idempotency_key,
            created_at: now,
            event,
        };
        let write = if let Some(binding) = batch_binding {
            TaskCoordinationRepo::create_project_plan_batch_coordination_request(
                &*self.db,
                create_input,
                binding,
            )
            .await
        } else {
            TaskCoordinationRepo::create_task_coordination_request(&*self.db, create_input).await
        };
        let (request, committed) = match write {
            Ok(write) => write,
            Err(db::DbError::CoordinationTurnBudgetExhausted) => {
                return Err(coordination_error(
                    "coordination.budget_exhausted",
                    "the Task coordination turn budget is exhausted",
                ));
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(event) = committed.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok(Some((request, committed.is_some())))
    }

    /// Durable scanner: completed check requests whose result needs triage and
    /// have no request yet. The request is the receipt, so a retry never
    /// charges another turn.
    pub async fn process_pending_validation_findings_requests(&self, limit: i64) -> Result<usize> {
        let ids: Vec<String> = sqlx::query_scalar(
            "SELECT req.id
             FROM validation_check_request req
             JOIN task_coordination c ON c.task_id=req.task_id
              AND c.authorization_version=req.authorization_version
             JOIN validation_run run ON run.id=req.candidate_run_id AND run.task_id=req.task_id
             LEFT JOIN validation_comparison cmp ON cmp.check_request_id=req.id
             WHERE req.state='completed' AND req.candidate_run_id IS NOT NULL
               AND req.trigger_kind IN ('coordinator_request','after_integration')
               AND c.turn_count < c.max_turns AND c.mode!='manual'
               AND c.revoked_at IS NULL AND c.team_stale=0
               AND c.coordinator_task_role_id IS NOT NULL
               AND c.coordinator_actor_kind='agent' AND c.coordinator_actor_id IS NOT NULL
               AND ((req.baseline=0 AND run.status IN ('failed','error','stale'))
                 OR (req.baseline=1 AND cmp.classification IN ('regression','inconclusive')))
               AND NOT EXISTS(
                 SELECT 1 FROM task_coordination_request r
                 WHERE r.task_id=req.task_id AND r.kind='validation_findings'
                   AND r.authorization_version=req.authorization_version
                   AND r.source_subject_id IN (run.id, cmp.id))
             ORDER BY req.updated_at, req.id LIMIT ?",
        )
        .bind(limit.clamp(1, 100))
        .fetch_all(self.db.pool())
        .await?;
        let mut created = 0;
        for id in ids {
            match self.request_validation_findings_for_check(&id).await {
                Ok(Some((_, true))) => created += 1,
                Ok(_) => {}
                Err(error) => tracing::warn!(
                    check_request_id = %id,
                    %error,
                    "validation findings request remains pending after a source-local error"
                ),
            }
        }
        Ok(created)
    }

    /// Exact source context for a `validation_findings` request. The stored
    /// source ids/digest must still match durable run and comparison rows.
    pub(crate) async fn validation_source_context(
        &self,
        request: &TaskCoordinationRequest,
    ) -> Result<Value> {
        let (kind, source_id, digest) = match (
            request.source_subject_kind.as_deref(),
            request.source_subject_id.as_deref(),
            request.source_subject_digest.as_deref(),
        ) {
            (Some(kind), Some(id), Some(digest)) => (kind, id, digest),
            _ => return Err(invalid_plan("request has no exact validation source")),
        };
        let (run, comparison) = match kind {
            "validation_run" => (
                ValidationRunRepo::get_validation_run(&*self.db, source_id).await?,
                None,
            ),
            "validation_comparison" => {
                let comparison =
                    ValidationComparisonRepo::get_validation_comparison(&*self.db, source_id)
                        .await?;
                let run = match comparison.as_ref() {
                    Some(comparison) => {
                        ValidationRunRepo::get_validation_run(
                            &*self.db,
                            &comparison.candidate_run_id,
                        )
                        .await?
                    }
                    None => None,
                };
                (run, comparison)
            }
            _ => return Err(invalid_plan("unsupported validation source kind")),
        };
        let run = run
            .filter(|run| run.task_id == request.task_id)
            .ok_or_else(|| ServiceError::not_found("validation_run", source_id.to_owned()))?;
        let run_context = validation_run_context(&self.db, &run).await?;
        if run_context["evidence_digest"].as_str() != Some(digest) {
            return Err(invalid_plan(
                "validation source digest no longer matches its Evidence",
            ));
        }
        let check = match run.check_request_id.as_deref() {
            Some(id) => {
                ValidationCheckRequestRepo::get_validation_check_request(&*self.db, id).await?
            }
            None => None,
        };
        let check_context = match check.as_ref() {
            Some(check) => {
                let definition = ValidationCatalogRepo::get_validation_check_definition(
                    &*self.db,
                    &check.check_definition_id,
                )
                .await?;
                json!({
                    "check_request_id": check.id,
                    "check_key": definition.map(|definition| definition.check_key),
                    "definition_digest": check.definition_digest,
                    "trigger": check.trigger_kind,
                    "baseline": check.baseline,
                    "reason": check.reason,
                    "subject": {
                        "kind": check.subject_kind,
                        "id": check.subject_id,
                        "version": check.subject_version,
                        "workspace_id": check.workspace_id,
                        "commit_sha": check.commit_sha,
                        "workspace_snapshot_digest": check.workspace_snapshot_digest,
                    },
                })
            }
            None => Value::Null,
        };
        Ok(json!({
            "source_subject_kind": kind,
            "source_subject_id": source_id,
            "run_digest": digest,
            "run": run_context,
            "comparison": comparison.as_ref().map(validation_comparison_context),
            "check": check_context,
        }))
    }

    pub(crate) async fn exact_validation_findings_context(
        &self,
        request: &TaskCoordinationRequest,
    ) -> Result<Value> {
        if request.kind != TaskCoordinationRequestKind::ValidationFindings
            || !matches!(
                request.state,
                TaskCoordinationRequestState::Requested
                    | TaskCoordinationRequestState::Running
                    | TaskCoordinationRequestState::Uncertain
            )
        {
            return Err(ServiceError::invalid_operation(
                "validation context requires a runnable ValidationFindings request",
            ));
        }
        self.validation_source_context(request).await
    }

    pub(crate) fn render_validation_findings_prompt(
        &self,
        request: &TaskCoordinationRequest,
        context: &Value,
    ) -> Result<String> {
        let run_id = request
            .source_subject_id
            .as_deref()
            .ok_or_else(|| invalid_plan("ValidationFindings request has no exact source"))?;
        let digest = request
            .source_subject_digest
            .as_deref()
            .ok_or_else(|| invalid_plan("ValidationFindings request has no exact digest"))?;
        let comparison_note =
            if request.source_subject_kind.as_deref() == Some("validation_comparison") {
                format!(
                    "comparison_id is {} and validation_run_id is the comparison's candidate run",
                    serde_json::to_string(run_id).unwrap()
                )
            } else {
                "comparison_id is null".to_owned()
            };
        Ok(format!(
            "You are the selected Task coordinator triaging one exact deterministic validation result. This execution is read-only and cannot change the result, a Gate, or the repository.\n\
Return exactly one JSON object and no markdown fences matching:\n\
{{\"schema_version\":2,\"request_id\":{request_id},\"task_id\":{task_id},\"authorization_version\":{authorization_version},\"validation_findings\":[{{\"validation_run_id\":\"...\",\"comparison_id\":null,\"run_digest\":{digest},\"disposition\":\"correct|recheck|human\",\"rationale\":\"...\",\"instructions\":\"...\",\"micro_fix\":null}}],\"outputs\":[]}}\n\
Exactly one decision. {comparison_note}. `correct` creates a bounded correction WorkUnit for the checked WorkUnit; `recheck` runs the pinned check again on the current exact subject; `human` asks a Human. Optional `outputs` (at most 4) may hold `request_validation` (check_key, subject{{kind,id,version}}, baseline, reason) or one `ask_human` (question, 2-6 options with id/label/description/consequence/authority_effect, optional recommended_option_id, recommendation_rationale, evidence_links) only with the `human` disposition; both require explicit TaskRole permission. micro_fix must be null. Never include commands, paths, commits or workspaces. Keep rationale under 8 KiB and instructions under 16 KiB.\n\
Exact source: {kind} {run_id} ({digest})\nContext: {context}",
            request_id = serde_json::to_string(&request.id).unwrap(),
            task_id = serde_json::to_string(&request.task_id).unwrap(),
            authorization_version = request.authorization_version,
            digest = serde_json::to_string(digest).unwrap(),
            kind = request.source_subject_kind.as_deref().unwrap_or("validation_run"),
            context = serde_json::to_string(context).unwrap_or_else(|_| "{}".to_owned()),
        ))
    }
}
