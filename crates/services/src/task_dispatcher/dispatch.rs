use db::{TaskLifecycleRepo, TaskLifecycleState, TaskRepo, WorkUnitRepo};

use crate::{task_lifecycle::TaskLifecycleService, Result};

use super::TaskDispatcher;

/// Per-Task admission ceiling used by automatic dispatch. WorkUnit-specific
/// concurrency (one active Execution per WorkUnit) remains authoritative; this
/// bounded Task-wide limit prevents a large DAG from consuming an unbounded
/// number of harness slots in one tick.
pub(super) const MAX_CONCURRENT_EXECUTIONS_PER_TASK: i64 = 4;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DispatchOutcome {
    pub admitted_execution_ids: Vec<String>,
    pub started_execution_ids: Vec<String>,
}

impl TaskDispatcher {
    /// Admit ready Agent WorkUnits for every Task whose durable dispatch intent
    /// is `running`. The dispatcher never allocates a Role, picks an unassigned
    /// Actor, or creates WorkUnits.
    pub(super) async fn dispatch_eligible_work_units(&self) -> Result<u64> {
        let dispatches = TaskLifecycleRepo::list_running_task_dispatches(&*self.db).await?;
        let mut admitted = 0;
        for dispatch in dispatches {
            if self.is_stopped() {
                break;
            }
            let outcome = self.dispatch_task_once(&dispatch.task_id).await?;
            admitted += outcome.admitted_execution_ids.len() as u64;
        }
        Ok(admitted)
    }

    /// Run one automatic dispatch tick for a single Task. Used by both the
    /// periodic dispatcher and the authenticated launch request, which reports
    /// admission separately from provider Start.
    pub async fn dispatch_task_once(&self, task_id: &str) -> Result<DispatchOutcome> {
        let Some(task) = TaskRepo::get_by_id(&*self.db, task_id, false).await? else {
            return Ok(DispatchOutcome::default());
        };
        let Some(dispatch) = TaskLifecycleRepo::get_task_dispatch(&*self.db, task_id).await? else {
            return Ok(DispatchOutcome::default());
        };
        if dispatch.state != db::TaskDispatchState::Running {
            return Ok(DispatchOutcome::default());
        }
        let Some(lifecycle) = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id).await?
        else {
            return Ok(DispatchOutcome::default());
        };
        if !matches!(
            lifecycle.state,
            TaskLifecycleState::Backlog | TaskLifecycleState::Ready | TaskLifecycleState::Active
        ) {
            return Ok(DispatchOutcome::default());
        }
        let running: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND status = 'running'",
        )
        .bind(&task.id)
        .fetch_one(self.db.pool())
        .await?;
        if running >= MAX_CONCURRENT_EXECUTIONS_PER_TASK {
            return Ok(DispatchOutcome::default());
        }
        let available_slots =
            usize::try_from(MAX_CONCURRENT_EXECUTIONS_PER_TASK - running).unwrap_or(0);
        let lifecycle_service = TaskLifecycleService::new(
            std::sync::Arc::clone(&self.db),
            std::sync::Arc::clone(&self.event_bus),
        );
        let preparation_user = dispatch
            .requested_by_id
            .as_deref()
            .filter(|_| dispatch.requested_by_kind == "human");
        if let (Some(user_id), Some(coordination)) = (
            preparation_user,
            db::TaskCoordinationRepo::get_task_coordination(&*self.db, &task.id).await?,
        ) {
            let blockers = self
                .task_service
                .orchestrator_work_unit_service()
                .prepare_coordination_workspaces(
                    &task.id,
                    user_id,
                    coordination.authorization_version,
                )
                .await?;
            for blocker in blockers {
                tracing::warn!(task_id = %task.id, %blocker, "guided workspace preparation deferred");
            }
        }
        let preflight = lifecycle_service.launch_preflight(&task.id).await?;
        let mut outcome = DispatchOutcome::default();
        for candidate in preflight
            .work_units
            .iter()
            .filter(|unit| unit.state == "ready")
            .take(available_slots)
        {
            if self.is_stopped() || outcome.admitted_execution_ids.len() >= available_slots {
                break;
            }
            let Some(work_unit) =
                WorkUnitRepo::get_by_id(&*self.db, &candidate.work_unit_id).await?
            else {
                continue;
            };
            let mut prompt = work_unit
                .execution_instructions
                .clone()
                .filter(|instructions| !instructions.trim().is_empty())
                .unwrap_or_else(|| automatic_work_unit_prompt(&task, &work_unit));
            if let Some(context) = work_unit_consumption_context(&self.db, &work_unit).await? {
                prompt.push_str("\n\n");
                prompt.push_str(&context);
            }
            let launch = self
                .task_service
                .launch_work_unit_execution(
                    &work_unit.id,
                    work_unit.version,
                    prompt,
                    Vec::new(),
                    None,
                )
                .await;
            match launch {
                Ok(launched) => {
                    outcome
                        .admitted_execution_ids
                        .push(launched.execution.id.clone());
                    match self
                        .task_service
                        .start_execution(launched.execution.id.clone())
                        .await
                    {
                        Ok(result) if result.accepted => outcome
                            .started_execution_ids
                            .push(launched.execution.id.clone()),
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(
                                task_id = %task.id,
                                work_unit_id = %work_unit.id,
                                execution_id = %launched.execution.id,
                                %error,
                                "automatic WorkUnit Execution was admitted but its provider Start is pending"
                            );
                        }
                    }
                }
                Err(error) => {
                    tracing::warn!(
                        task_id = %task.id,
                        work_unit_id = %work_unit.id,
                        %error,
                        "automatic WorkUnit admission failed"
                    );
                }
            }
        }
        if !outcome.admitted_execution_ids.is_empty() {
            tracing::info!(
                task_id = %task.id,
                admitted = outcome.admitted_execution_ids.len(),
                "task dispatcher admitted WorkUnits"
            );
        }
        Ok(outcome)
    }
}

async fn work_unit_consumption_context(
    db: &db::SqliteDb,
    work_unit: &db::WorkUnit,
) -> Result<Option<String>> {
    use db::{CollaborationRepo, ExecutionRepo, WorkspaceRepo};
    use sqlx::Row;
    let mut sections = Vec::new();
    if let Some(subject) = crate::task_rework::get_review_subject(db, &work_unit.id).await? {
        sections.push("You are performing formal independent review. Inspect the exact result against its scope and acceptance criteria. Do not edit files, implement fixes, approve your own reasoning as evidence, or contact external providers. Report only concrete findings with reproduction/evidence; distinguish unavailable checks from passes. End your assistant output with exactly one final line in this contract: FORGE_RESULT: {\"schema_version\":1,\"kind\":\"review\",\"verdict\":\"pass|fail|needs_human\",\"summary\":\"nonempty factual summary\",\"criteria\":[\"criterion checked\"],\"findings\":[\"concrete defect with location and evidence\"],\"questions\":[],\"evidence_considered\":[]}. Choose one verdict. Use fail for confirmed defects, pass only when the required criteria are met, and needs_human for an unresolved decision or missing essential evidence. findings and questions must be JSON arrays; findings must be concrete strings, not instructions to silently change scope.".to_owned());
        sections.push(format!(
            "Exact review subject: WorkUnit {} v{}, result Execution {}, commit {}, Workspace {}, snapshot digest {}. Review this immutable result independently. Your current directory is an isolated review context; the implementation has not been copied into it. Do not edit the result or reuse its native session.",
            subject.reviewed_work_unit_id,
            subject.reviewed_work_unit_version.map(|v|v.to_string()).unwrap_or_else(||"pending".to_owned()),
            subject.result_execution_id.as_deref().unwrap_or("none"),subject.result_commit_sha.as_deref().unwrap_or("none"),
            subject.result_workspace_id.as_deref().unwrap_or("none"),subject.workspace_snapshot_digest.as_deref().unwrap_or("none")));
        if let Some(unit) = WorkUnitRepo::get_by_id(db, &subject.reviewed_work_unit_id).await? {
            sections.push(format!("Reviewed scope and acceptance:\n{}", unit.scope));
        }
        if let Some(id) = subject.result_execution_id.as_deref() {
            if let Some(result) = ExecutionRepo::get_by_id(db, id).await? {
                sections.push(format!(
                    "Delivery summary from exact result Execution {id}:\n{}",
                    result.summary.as_deref().unwrap_or("No summary recorded")
                ));
            }
        }
        if let Some(id) = subject.result_workspace_id.as_deref() {
            if let Some(workspace) = WorkspaceRepo::get_by_id(db, id).await? {
                if workspace.task_id != work_unit.task_id {
                    return Err(crate::ServiceError::invalid_operation(
                        "Review workspace belongs to another Task",
                    ));
                }
                sections.push(format!("Read the exact implementation at {} (commit {}). Read access does not grant a WorkspaceLease or write permission.",workspace.worktree_path,subject.result_commit_sha.as_deref().unwrap_or("none")));
            }
        }
        if let Some(id) = subject.result_artifact_id.as_deref() {
            let artifact = CollaborationRepo::get_artifact(db, id)
                .await?
                .filter(|a| a.task_id == work_unit.task_id)
                .ok_or_else(|| {
                    crate::ServiceError::invalid_operation(
                        "Exact review delivery Artifact is unavailable",
                    )
                })?;
            sections.push(format!(
                "Exact delivery Artifact {id}, digest {}:\n{}",
                artifact.digest.as_deref().unwrap_or("none"),
                artifact
                    .content
                    .as_deref()
                    .unwrap_or("External Artifact: inspect its explicit storage reference")
            ));
        }
    } else {
        let requirement: Option<(String,i64,String,Option<String>,Option<String>)>=sqlx::query_as("SELECT source_work_unit_id,source_work_unit_version,result_execution_id,result_commit_sha,result_workspace_id FROM work_unit_result_requirement WHERE work_unit_id=?")
            .bind(&work_unit.id).fetch_optional(db.pool()).await?;
        if let Some((id, version, execution, commit, workspace)) = requirement {
            sections.push(format!("Exact consumed result: WorkUnit {id} v{version}, result Execution {execution}, commit {}, Workspace {}. Start from this exact result; do not modify sibling WorkUnits.",commit.as_deref().unwrap_or("none"),workspace.as_deref().unwrap_or("none")));
        }
    }
    let inputs=sqlx::query("SELECT pin.artifact_id,pin.artifact_digest,pin.producer_execution_id,a.digest,a.content,a.content_ref,producer.execution_id AS actual_producer FROM work_unit_artifact_input pin LEFT JOIN artifact a ON a.id=pin.artifact_id AND a.task_id=pin.task_id LEFT JOIN artifact_execution_producer producer ON producer.artifact_id=a.id WHERE pin.work_unit_id=? ORDER BY pin.artifact_id")
        .bind(&work_unit.id).fetch_all(db.pool()).await?;
    for row in inputs {
        let id: String = row.try_get("artifact_id")?;
        let expected: String = row.try_get("artifact_digest")?;
        if row.try_get::<Option<String>, _>("digest")?.as_deref() != Some(expected.as_str())
            || row
                .try_get::<Option<String>, _>("actual_producer")?
                .as_deref()
                != Some(row.try_get::<String, _>("producer_execution_id")?.as_str())
        {
            return Err(crate::ServiceError::invalid_operation(
                "A pinned input Artifact or its producer no longer matches the accepted package",
            ));
        }
        let content: Option<String> = row.try_get("content")?;
        let reference: Option<String> = row.try_get("content_ref")?;
        sections.push(format!(
            "Explicit input Artifact {id}, digest {expected}:\n{}",
            content
                .or(reference.map(|r| format!("External content reference: {r}")))
                .unwrap_or_default()
        ));
    }
    let context = sections.join("\n\n");
    if context.len() > 128 * 1024 {
        return Err(crate::ServiceError::invalid_operation("WorkUnit handoff exceeds the explicit context budget; narrow its Artifact package instead of silently truncating evidence"));
    }
    Ok((!context.is_empty()).then_some(context))
}

fn automatic_work_unit_prompt(task: &db::Task, work_unit: &db::WorkUnit) -> String {
    let brief = task
        .description
        .as_deref()
        .map(str::trim)
        .filter(|description| !description.is_empty())
        .unwrap_or(&task.title);
    format!(
        "You are executing one WorkUnit for Task \"{}\".\n\n\
         Task brief:\n{}\n\n\
         WorkUnit ({} v{}, role {}):\n{}\n\n\
         Complete exactly this WorkUnit scope. Do not integrate, merge, publish, or \
         modify sibling WorkUnits. A repository WorkUnit must leave its committed result \
         in its exact Workspace; integration is a separate explicit operation. If a \
         repository change is not required, do not claim one. Report concrete results, \
         findings, and any blocker that prevents completion.",
        task.title,
        brief,
        work_unit.title,
        work_unit.version,
        work_unit.role,
        work_unit.scope.trim(),
    )
}

#[cfg(test)]
mod tests {
    use super::automatic_work_unit_prompt;

    #[test]
    fn automatic_prompt_names_task_and_exact_work_unit_without_purpose() {
        let task = db::Task {
            id: "task".to_owned(),
            project_id: "project".to_owned(),
            repo_id: None,
            parent_task_id: None,
            title: "Ship the API".to_owned(),
            description: Some("Keep the wire contract exact.".to_owned()),
            lifecycle_state: db::TaskLifecycleState::Ready,
            is_automation: false,
            priority: 0,
            board_position: 0.0,
            subtask_order: None,
            merge_config: None,
            metadata_json: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            deleted_at: None,
            version: 1,
            created_at: "2026-11-05T00:00:00Z".to_owned(),
            updated_at: "2026-11-05T00:00:00Z".to_owned(),
        };
        let unit = db::WorkUnit {
            id: "unit".to_owned(),
            task_id: "task".to_owned(),
            parent_work_unit_id: None,
            title: "Implement endpoint".to_owned(),
            scope: "Add the route and tests.".to_owned(),
            execution_instructions: None,
            status: db::WorkUnitStatus::Open,
            role: "implementer".to_owned(),
            assigned_actor: Some(db::ActorRef::Agent("agent".to_owned())),
            requires_integration: false,
            provenance: None,
            review_approval_gate_id: None,
            review_approval_line_id: None,
            created_by: db::ActorRef::Human("user".to_owned()),
            version: 3,
            created_at: "2026-11-05T00:00:00Z".to_owned(),
            updated_at: "2026-11-05T00:00:00Z".to_owned(),
        };
        let prompt = automatic_work_unit_prompt(&task, &unit);
        assert!(prompt.contains("\"Ship the API\""));
        assert!(prompt.contains("Implement endpoint"));
        assert!(prompt.contains("v3"));
        assert!(!prompt.to_ascii_lowercase().contains("purpose"));
    }
}
