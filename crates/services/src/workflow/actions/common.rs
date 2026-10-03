use std::sync::Arc;

use db::{
    new_uuid_v4, now_rfc3339, ActorKind, CommentAuthorType, CreateTaskComment, DbError, Execution,
    ExecutionRepo, PageRequest, RoleMembershipStatus, SortBy, SortOrder, TaskCommentRepo, TaskRepo,
    TaskRoleAssignment, TaskRoleAssignmentRepo, TransitionLog, TransitionLogRepo, UpdateTask,
    WorkspaceRepo,
};
use events::{event_timestamp, EventContext, ForgeEvent};
use serde_json::{json, Value};

use crate::workflow::{
    default_states, engine::WorkflowEngine, inherited_subtask_workflow, HookContext, HookResult,
};

pub(super) async fn get_role_assignment(
    ctx: &HookContext,
    role: &str,
) -> Result<Option<TaskRoleAssignment>, String> {
    if let Some(canonical_role) = db::canonical_task_role_name(role) {
        if let Some(members) = crate::task_service::current_role_memberships_authoritative(
            &ctx.db,
            &ctx.task_id,
            &canonical_role,
        )
        .await
        .map_err(|error| error.to_string())?
        {
            // The pre-WorkUnit executor has one launch slot. Select a concrete
            // active Agent deterministically when present; a Human is selected
            // only when no Agent can be launched. Eligibility still comes from
            // the complete membership set, never from the legacy projection.
            let member = members
                .into_iter()
                .filter(|member| member.status == RoleMembershipStatus::Active)
                .min_by_key(|member| {
                    (
                        if member.actor_kind == ActorKind::Agent {
                            0
                        } else {
                            1
                        },
                        member.created_at.clone(),
                        member.id.clone(),
                    )
                });
            return Ok(member.map(|member| TaskRoleAssignment {
                id: member.id,
                task_id: ctx.task_id.clone(),
                role_name: role.to_owned(),
                assignee_type: Some(match member.actor_kind {
                    ActorKind::Agent => db::AssigneeKind::Agent,
                    ActorKind::Human => db::AssigneeKind::User,
                }),
                assignee_id: Some(member.actor_id),
                created_at: member.created_at,
                updated_at: member.updated_at,
            }));
        }
    }
    match TaskRoleAssignmentRepo::get_by_task_and_role(&*ctx.db, &ctx.task_id, role).await {
        Ok(assignment) => Ok(assignment),
        Err(DbError::NotFound) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Resolve an Agent for a workflow launch. Unlike the display-oriented
/// compatibility object above, this path must skip paused, unavailable, or
/// full Agents and continue through the authoritative membership set.
pub(super) async fn get_usable_agent_assignment(
    ctx: &HookContext,
    role: &str,
) -> Result<Option<TaskRoleAssignment>, String> {
    if let Some(canonical_role) = db::canonical_task_role_name(role) {
        if let Some(memberships) = crate::task_service::current_role_memberships_authoritative(
            &ctx.db,
            &ctx.task_id,
            &canonical_role,
        )
        .await
        .map_err(|error| error.to_string())?
        {
            let repository_required = TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
                .await
                .map_err(|error| error.to_string())?
                .is_some_and(|task| task.repo_id.is_some());
            let selected = if repository_required {
                crate::task_service::select_usable_repository_agent_id(
                    &ctx.db,
                    &ctx.project_id,
                    &memberships,
                )
                .await
                .map_err(|error| error.to_string())?
            } else {
                crate::task_service::select_usable_agent_id(&ctx.db, &memberships)
                    .await
                    .map_err(|error| error.to_string())?
            };
            let Some(agent_id) = selected else {
                if memberships.iter().any(|member| {
                    member.status == RoleMembershipStatus::Active
                        && member.actor_kind == ActorKind::Agent
                }) {
                    return Ok(None);
                }
                return Ok(memberships
                    .into_iter()
                    .filter(|member| {
                        member.status == RoleMembershipStatus::Active
                            && member.actor_kind == ActorKind::Human
                    })
                    .min_by_key(|member| (member.created_at.clone(), member.id.clone()))
                    .map(|member| TaskRoleAssignment {
                        id: member.id,
                        task_id: ctx.task_id.clone(),
                        role_name: role.to_owned(),
                        assignee_type: Some(db::AssigneeKind::User),
                        assignee_id: Some(member.actor_id),
                        created_at: member.created_at,
                        updated_at: member.updated_at,
                    }));
            };
            return Ok(memberships
                .into_iter()
                .find(|member| {
                    member.status == RoleMembershipStatus::Active
                        && member.actor_kind == ActorKind::Agent
                        && member.actor_id == agent_id
                })
                .map(|member| TaskRoleAssignment {
                    id: member.id,
                    task_id: ctx.task_id.clone(),
                    role_name: role.to_owned(),
                    assignee_type: Some(db::AssigneeKind::Agent),
                    assignee_id: Some(member.actor_id),
                    created_at: member.created_at,
                    updated_at: member.updated_at,
                }));
        }
    }
    match TaskRoleAssignmentRepo::get_by_task_and_role(&*ctx.db, &ctx.task_id, role).await {
        Ok(assignment) => Ok(assignment),
        Err(DbError::NotFound) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn execution_guard_roles(role: &str) -> Vec<&str> {
    let mut roles = vec![role];
    if role == crate::workflow::default_roles::CODER {
        roles.push("executor");
    }
    roles
}

pub(super) async fn has_running_execution_for_roles(
    ctx: &HookContext,
    roles: &[&str],
) -> Result<bool, String> {
    let page = ExecutionRepo::list_by_task(
        &*ctx.db,
        &ctx.task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(page.items.iter().any(|execution| {
        execution.work_unit_id.is_none()
            && execution.status == db::ExecutionStatus::Running
            && roles.iter().any(|role| execution.role == *role)
    }))
}

pub(super) async fn task(ctx: &HookContext) -> Result<db::Task, String> {
    TaskRepo::get_by_id(&*ctx.db, &ctx.task_id, false)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("task not found: {}", ctx.task_id))
}

pub(super) async fn latest_executor_execution(ctx: &HookContext) -> Option<Execution> {
    let page = ExecutionRepo::list_by_task(
        &*ctx.db,
        &ctx.task_id,
        PageRequest {
            cursor: None,
            limit: 20,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await
    .ok()?;
    page.items.into_iter().find(|execution| {
        execution.work_unit_id.is_none()
            && matches!(execution.role.as_str(), "executor" | "coder" | "worker")
    })
}

pub(super) async fn workspace_id(ctx: &HookContext) -> Option<String> {
    if let Some(workspace_id) = ctx.workspace_id.clone() {
        return Some(workspace_id);
    }
    if let Some(execution) = latest_executor_execution(ctx).await {
        if execution.workspace_id.is_some() {
            return execution.workspace_id;
        }
    }
    WorkspaceRepo::get_by_task_id(&*ctx.db, &ctx.task_id)
        .await
        .ok()
        .flatten()
        .map(|workspace| workspace.id)
}

pub(super) async fn transition_subtask_with_inherited_workflow(
    ctx: &HookContext,
    subtask: db::Task,
    target_state: &str,
) -> Result<(), String> {
    let workflow = inherited_subtask_workflow();
    let engine = WorkflowEngine {
        db: Arc::clone(&ctx.db),
        event_bus: Arc::clone(&ctx.event_bus),
        review_runner: ctx.review_runner.clone(),
        merge_service: ctx.merge_service.clone(),
        cleanup_scheduler: ctx.cleanup_scheduler.clone(),
        task_executor: ctx.task_executor.clone(),
        adapter_registry: ctx.adapter_registry.clone(),
        daemon_connections: ctx.daemon_connections.clone(),
        workspace_exec_locks: ctx.workspace_exec_locks.clone(),
        terminal_activity: ctx.terminal_activity.clone(),
        workspace_root: ctx.workspace_root.clone(),
        repo_cache_locks: ctx.repo_cache_locks.clone(),
    };
    let mut current = subtask;

    if target_state == default_states::DONE && current.status == default_states::TODO {
        current = engine
            .transition(
                &current.id,
                default_states::IN_PROGRESS,
                current.version,
                &workflow,
                &api_types::Actor::system(api_types::SystemComponent::Workflow),
                "root done propagation",
                false,
            )
            .await
            .map_err(|error| error.to_string())?
            .task;
    }

    engine
        .transition(
            &current.id,
            target_state,
            current.version,
            &workflow,
            &api_types::Actor::system(api_types::SystemComponent::Workflow),
            "root subtask cascade",
            false,
        )
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn merge_fix_budget_result(ctx: &HookContext) -> Option<HookResult> {
    let task = match task(ctx).await {
        Ok(task) => task,
        Err(reason) => return Some(HookResult::Failed { reason }),
    };
    let budget = match crate::task_service::config::runtime_retry_budget(
        &task,
        crate::task_service::config::RetryBudgetKind::MergeFix,
        Some(&ctx.state_config),
        ctx.gate_config.as_ref(),
    ) {
        Ok(budget) => budget,
        Err(error) => {
            return Some(HookResult::Failed {
                reason: error.to_string(),
            });
        }
    };
    let count = match TransitionLogRepo::list_by_task(&*ctx.db, &ctx.task_id).await {
        Ok(entries) => merge_fix_rejections_since_boundary(&entries),
        Err(error) => {
            return Some(HookResult::Failed {
                reason: error.to_string(),
            });
        }
    };
    // This runs after `merging -> merge_failed` has been logged. The current
    // merge_failed entry consumes one allowed merge-fix follow-up, so exhaustion
    // is count > budget here; budget=0 blocks on the first conflict.
    if count > i64::from(budget) {
        let reason = "merge-fix follow-up failed: conflict";
        if let Err(error) = block_task(
            ctx,
            &task,
            reason,
            api_types::FailureKind::MergeConflict,
            None,
        )
        .await
        {
            return Some(HookResult::Failed {
                reason: error.to_string(),
            });
        }
        Some(HookResult::Ok)
    } else {
        None
    }
}

pub(super) fn merge_fix_rejections_since_boundary(entries: &[TransitionLog]) -> i64 {
    let boundary = entries.iter().rposition(|entry| {
        entry.from_state == default_states::MERGING
            && !entry.rejection
            && (entry.to_state != default_states::MERGING
                || entry.trigger_name.as_deref() == Some("reset_retry_window"))
    });
    let entries = boundary
        .and_then(|index| entries.get(index + 1..))
        .unwrap_or(entries);
    entries
        .iter()
        .filter(|entry| {
            entry.from_state == default_states::MERGING
                && entry.to_state == default_states::MERGE_FAILED
                && entry.rejection
        })
        .count() as i64
}

pub(super) fn follow_up_trigger(ctx: &HookContext) -> &'static str {
    if ctx.to_state == default_states::MERGE_FAILED
        || ctx.from_state == default_states::MERGE_FAILED
    {
        "merge_failed"
    } else if ctx.from_state == default_states::REVIEW {
        "review_failed"
    } else {
        "role_follow_up"
    }
}

pub(super) async fn create_system_comment(ctx: &HookContext, content: String) -> db::Result<()> {
    let now = now_rfc3339();
    let comment = TaskCommentRepo::create_comment(
        &*ctx.db,
        CreateTaskComment {
            id: new_uuid_v4(),
            task_id: ctx.task_id.clone(),
            author_type: CommentAuthorType::System,
            author_id: None,
            author_name: "Forge".to_string(),
            content,
            created_at: now.clone(),
            updated_at: now,
        },
    )
    .await?;
    let memory_service = crate::MemoryService::new(Arc::clone(&ctx.db));
    if let Err(error) = memory_service
        .record_task_comment(&ctx.project_id, &comment)
        .await
    {
        tracing::warn!(error = %error, "memory indexing failed (non-fatal)");
    }
    ctx.event_bus.publish(ForgeEvent {
        event_type: "comment.created".to_string(),
        entity_id: comment.id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::CommentCreated {
            task_id: ctx.task_id.clone(),
            comment_id: comment.id,
            author_type: "system".to_string(),
            author_name: "Forge".to_string(),
        },
    });
    Ok(())
}

pub(super) async fn persist_merge_error(
    ctx: &HookContext,
    task: &db::Task,
    error_type: api_types::FailureKind,
    message: &str,
) -> db::Result<()> {
    let detected_at = now_rfc3339();
    let annotation = json!({
        "type": error_type,
        "message": message,
        "detected_at": detected_at,
    });
    TaskRepo::update(
        &*ctx.db,
        UpdateTask {
            id: task.id.clone(),
            expected_version: task.version,
            title: None,
            description: None,
            priority: None,
            merge_config: None,
            error_annotation: Some(Some(annotation.to_string())),
            blocked_json: None,
            failed_json: None,
            task_state_config: None,
            parent_task_id: None,
            updated_at: now_rfc3339(),
        },
    )
    .await?;
    Ok(())
}

pub(super) async fn persist_target_repo_dirty_error(
    ctx: &HookContext,
    task: &db::Task,
    message: &str,
    _files: &[String],
) -> db::Result<()> {
    persist_merge_error(ctx, task, api_types::FailureKind::TargetRepoDirty, message).await
}

pub(super) async fn block_task(
    ctx: &HookContext,
    task: &db::Task,
    reason: &str,
    kind: api_types::FailureKind,
    source: Option<&str>,
) -> db::Result<()> {
    let now = now_rfc3339();
    let blocked_meta = json!({
        "reason": reason,
        "created_at": now.clone(),
        "kind": kind,
        "source": source,
        "execution_id": ctx.execution_id.clone(),
    });
    let mut current = task.clone();
    for attempt in 0..3 {
        match TaskRepo::update(
            &*ctx.db,
            UpdateTask {
                id: current.id.clone(),
                expected_version: current.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                error_annotation: None,
                blocked_json: Some(Some(blocked_meta.to_string())),
                failed_json: Some(None),
                task_state_config: None,
                parent_task_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        {
            Ok(_) => {
                tracing::info!(
                    task_id = %ctx.task_id,
                    status = %task.status,
                    kind = %kind,
                    reason = %reason,
                    source = ?source,
                    execution_id = ?ctx.execution_id,
                    "task blocked"
                );
                break;
            }
            Err(DbError::VersionConflict) if attempt < 2 => {
                current = TaskRepo::get_by_id(&*ctx.db, &task.id, false)
                    .await?
                    .ok_or(DbError::NotFound)?;
            }
            Err(error) => return Err(error),
        }
    }
    ctx.event_bus.publish(ForgeEvent {
        event_type: "task.blocked".to_string(),
        entity_id: ctx.task_id.clone(),
        timestamp: event_timestamp(),
        context: EventContext::TaskBlocked {
            project_id: ctx.project_id.clone(),
            reason: reason.to_string(),
            kind: Some(kind),
            source: source.map(str::to_string),
            execution_id: ctx.execution_id.clone(),
        },
    });
    Ok(())
}

pub(super) fn review_ci_steps(value: &Value) -> Result<Vec<String>, String> {
    let value = value.get("review").unwrap_or(value);
    match value.get("ci_steps") {
        Some(steps) => {
            let Some(steps) = steps.as_array() else {
                return Err("review ci_steps must be an array".to_string());
            };
            steps
                .iter()
                .map(|step| {
                    step.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "review ci_steps entries must be strings".to_string())
                })
                .collect()
        }
        None => Ok(Vec::new()),
    }
}
