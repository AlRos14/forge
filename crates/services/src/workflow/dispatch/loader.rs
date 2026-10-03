use std::sync::Arc;

use api_types::{StateKind, WorkflowDefinition};
use db::{
    Artifact, CollaborationRepo, CollaborationTarget, ExecutionRepo, ExecutionStatus, PageRequest,
    SortBy, SortOrder, TaskCommentRepo, TaskRepo, TaskRoleRepo, TransitionLogRepo,
    ValidationRunRepo, WorkspaceRepo,
};
use serde_json::Value;

use crate::workflow::dispatch::EXECUTION_POLICY_RESUME_LATEST_TARGET_ROLE_THREAD;
use crate::{workflow::dispatch::AgentDispatchContext, Result, ServiceError};

const CONTEXT_PROJECTION_LIMIT: usize = 16_000;

pub async fn load_agent_dispatch_context(
    db: Arc<db::SqliteDb>,
    task_id: &str,
    role: &str,
    state_name: &str,
    state_config: Value,
    execution_policy: Option<&str>,
    causing_execution_id: Option<&str>,
    workflow: &WorkflowDefinition,
) -> Result<AgentDispatchContext> {
    let task = TaskRepo::get_by_id(&*db, task_id, false)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task_id.to_string()))?;
    let transition_log = TransitionLogRepo::list_by_task(&*db, task_id).await?;
    let comments = TaskCommentRepo::list_comments(
        &*db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Asc,
        },
    )
    .await?
    .items;
    let collaboration_context = load_collaboration_context(&db, task_id, role).await?;
    let parent_task = match task.parent_task_id.as_deref() {
        Some(parent_task_id) => TaskRepo::get_by_id(&*db, parent_task_id, false).await?,
        None => None,
    };
    let sub_tasks = load_sub_tasks(&db, task_id).await?;
    let last_manual_bounce_reason =
        derive_last_manual_bounce_reason(&transition_log, state_name, workflow);
    // This legacy workflow policy selects a causal/contextual parent only.
    // Follow-up creation performs the Actor-first, explicit HarnessSession
    // continuity check; this lookup is never a session selector.
    let causing_execution = match causing_execution_id {
        Some(id) => Some(
            ExecutionRepo::get_by_id(&*db, id)
                .await?
                .filter(|execution| execution.task_id == task_id)
                .ok_or_else(|| ServiceError::not_found("execution", id.to_owned()))?,
        ),
        None => None,
    };
    // Review rework starts a fresh coder Execution. The exact Review Execution
    // and attached ReviewReport are context, never permission to infer a prior
    // coder Execution or HarnessSession from role recency. Reviewer dispatch
    // also never resumes a prior thread by role recency.
    let review_rework = causing_execution.as_ref().is_some_and(|execution| {
        execution.role == crate::workflow::default_roles::REVIEWER
            && execution.purpose == Some(db::ExecutionPurpose::Review)
    });
    let continuation_execution =
        if should_resume_latest_target_role_thread(execution_policy, role, review_rework) {
            latest_terminal_execution_for_role(&db, task_id, role).await?
        } else {
            None
        };
    let continuation_of_execution_id = continuation_execution
        .as_ref()
        .map(|execution| execution.id.clone());
    let continuation_logs_path = continuation_execution
        .as_ref()
        .and_then(|execution| execution.logs_path.clone());
    let plan_artifacts = match causing_execution_id {
        Some(execution_id) => {
            crate::plan_artifact::plan_artifacts_for_execution(&db, task_id, execution_id).await?
        }
        None => Vec::new(),
    };
    let plan_artifact_ids = plan_artifacts
        .iter()
        .map(|artifact| artifact.id.clone())
        .collect::<Vec<_>>();
    let plan = render_plan_context(&plan_artifacts);
    let review_evidence = if role == crate::workflow::default_roles::REVIEWER {
        match crate::DiffService::new(Arc::clone(&db))
            .task_diff(task_id)
            .await
        {
            Ok(diff) => {
                let validation_context =
                    load_validation_context(&db, task_id, diff.head_sha.as_str()).await?;
                Some(format!(
                    "Base SHA: {}\nHead SHA: {}\n\nExact diff:\n```diff\n{}\n```{}",
                    diff.base_sha,
                    diff.head_sha,
                    diff.diff,
                    validation_context.unwrap_or_default(),
                ))
            }
            Err(error) => Some(format!("Diff evidence unavailable: {error}")),
        }
    } else {
        None
    };

    Ok(AgentDispatchContext {
        task,
        role: role.to_string(),
        state_name: state_name.to_string(),
        state_config,
        transition_log,
        comments,
        plan,
        plan_artifact_ids,
        review_evidence,
        prior_reviews: Vec::new(),
        parent_task,
        sub_tasks,
        last_manual_bounce_reason,
        continuation_of_execution_id,
        continuation_logs_path,
        latest_review_feedback: collaboration_context,
        latest_review_execution_id: None,
        latest_review_logs_path: None,
    })
}

fn render_plan_context(artifacts: &[Artifact]) -> Option<String> {
    if artifacts.is_empty() {
        return None;
    }
    Some(
        artifacts
            .iter()
            .map(|artifact| {
                let (producer, actor) = match &artifact.producer {
                    db::ArtifactProducer::Execution { execution_id, actor } => {
                        let actor = match actor {
                            db::ActorRef::Human(id) => format!("human:{id}"),
                            db::ActorRef::Agent(id) => format!("agent:{id}"),
                        };
                        (format!("execution:{execution_id}"), actor)
                    }
                    db::ArtifactProducer::ValidationRun { validation_run_id } => {
                        (format!("validation_run:{validation_run_id}"), "none".to_owned())
                    }
                };
                format!(
                    "Plan Artifact {}\nDigest: {}\nProducer: {}\nProducer Actor: {}\nCreated: {}\n\n{}",
                    artifact.id,
                    artifact.digest.as_deref().unwrap_or("unavailable"),
                    producer,
                    actor,
                    artifact.created_at,
                    artifact.content.as_deref().unwrap_or("[content unavailable]")
                )
            })
            .collect::<Vec<_>>()
            .join("\n\n---\n\n"),
    )
}

fn should_resume_latest_target_role_thread(
    execution_policy: Option<&str>,
    role: &str,
    review_rework: bool,
) -> bool {
    role != crate::workflow::default_roles::REVIEWER
        && !review_rework
        && execution_policy == Some(EXECUTION_POLICY_RESUME_LATEST_TARGET_ROLE_THREAD)
}

fn derive_last_manual_bounce_reason(
    transition_log: &[db::TransitionLog],
    state_name: &str,
    workflow: &WorkflowDefinition,
) -> Option<String> {
    transition_log
        .iter()
        .rev()
        .find(|entry| {
            entry.to_state == state_name
                && !entry.rejection
                && workflow
                    .states
                    .iter()
                    .any(|state| state.name == entry.from_state && state.kind == StateKind::Gate)
        })
        .map(|entry| entry.trigger_reason.clone())
}

async fn load_sub_tasks(db: &db::SqliteDb, parent_task_id: &str) -> Result<Vec<db::Task>> {
    Ok(TaskRepo::list_subtasks_ordered(db, parent_task_id).await?)
}

async fn latest_terminal_execution_for_role(
    db: &db::SqliteDb,
    task_id: &str,
    role: &str,
) -> Result<Option<db::Execution>> {
    if let Some(execution) = latest_terminal_execution_for_exact_role(db, task_id, role).await? {
        return Ok(Some(execution));
    }
    if role == crate::workflow::default_roles::CODER {
        return latest_terminal_execution_for_exact_role(db, task_id, "executor").await;
    }
    Ok(None)
}

async fn latest_terminal_execution_for_exact_role(
    db: &db::SqliteDb,
    task_id: &str,
    role: &str,
) -> Result<Option<db::Execution>> {
    let page = ExecutionRepo::list_by_task_and_role(
        db,
        task_id,
        role,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;

    Ok(page.items.into_iter().find(|execution| {
        execution.work_unit_id.is_none()
            && matches!(
                execution.status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
            )
    }))
}

async fn load_collaboration_context(
    db: &db::SqliteDb,
    task_id: &str,
    role: &str,
) -> Result<Option<String>> {
    let target_role_id = TaskRoleRepo::get_by_task_and_role(db, task_id, role)
        .await?
        .map(|role| role.id);
    let messages = CollaborationRepo::list_messages(
        db,
        task_id,
        PageRequest {
            cursor: None,
            limit: 100,
            include_total: false,
            sort_by: SortBy::CreatedAt,
            sort_order: SortOrder::Desc,
        },
    )
    .await?;
    let mut entries = Vec::new();
    for message in messages.items {
        let addressed = match &message.target {
            CollaborationTarget::Task => true,
            CollaborationTarget::Role(role_id) => target_role_id.as_deref() == Some(role_id),
            CollaborationTarget::Actor(_) => false,
        };
        if !addressed {
            continue;
        }
        let mut entry = format!("Message {}: {}", message.id, message.body);
        for artifact_id in message.artifact_ids {
            let Some(artifact) = CollaborationRepo::get_artifact(db, &artifact_id).await? else {
                continue;
            };
            if artifact.task_id != task_id {
                return Err(ServiceError::invalid_operation(
                    "collaboration Message references an Artifact from another Task",
                ));
            }
            entry.push_str(&format!(
                "\nAttached Artifact {} ({:?}, digest {}):\n{}",
                artifact.id,
                artifact.kind,
                artifact.digest.as_deref().unwrap_or("unavailable"),
                artifact
                    .content
                    .as_deref()
                    .unwrap_or("content stored externally"),
            ));
        }
        entries.push(entry);
        if entries.len() == 5 {
            break;
        }
    }
    entries.reverse();
    if entries.is_empty() {
        return Ok(None);
    }
    let context = entries.join("\n\n");
    Ok(Some(tail_chars(&context, CONTEXT_PROJECTION_LIMIT)))
}

async fn load_validation_context(
    db: &db::SqliteDb,
    task_id: &str,
    head_sha: &str,
) -> Result<Option<String>> {
    let Some(workspace) = WorkspaceRepo::get_by_task_id(db, task_id).await? else {
        return Ok(None);
    };
    let snapshot_digest =
        crate::ValidationService::snapshot_digest(&workspace.worktree_path).await?;
    let runs = ValidationRunRepo::list_validation_runs_by_task(db, task_id).await?;
    let exact = runs
        .into_iter()
        .filter(|run| {
            run.workspace_id == workspace.id
                && run.commit_sha == head_sha
                && run.workspace_snapshot_digest == snapshot_digest
        })
        .collect::<Vec<_>>();
    if exact.is_empty() {
        return Ok(None);
    }
    let mut context = format!(
        "\n\nDeterministic Validation Evidence for exact Workspace {}, commit {}, and snapshot {}:\n",
        workspace.id, head_sha, snapshot_digest
    );
    for run in exact {
        context.push_str(&format!(
            "\nValidationRun {}: check={}, status={}, config_digest={}\n",
            run.id, run.check_identity, run.status, run.config_digest
        ));
        for evidence in ValidationRunRepo::list_evidence_for_validation_run(db, &run.id).await? {
            let content: Value = serde_json::from_str(&evidence.content_json).map_err(|_| {
                ServiceError::invalid_operation("stored Validation Evidence is invalid JSON")
            })?;
            context.push_str(&format!(
                "Evidence {} (kind={}, digest={}): {}\n",
                evidence.id, evidence.kind, evidence.digest, content
            ));
        }
        if context.len() >= CONTEXT_PROJECTION_LIMIT {
            break;
        }
    }
    Ok(Some(tail_chars(&context, CONTEXT_PROJECTION_LIMIT)))
}

fn tail_chars(value: &str, limit: usize) -> String {
    if value.len() <= limit {
        return value.to_owned();
    }

    let mut start = value.len() - limit;
    while !value.is_char_boundary(start) {
        start += 1;
    }
    format!(
        "[truncated projection; exact source remains addressable by ID]\n{}",
        &value[start..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{CreateProject, CreateTask, ProjectRepo, TaskRepo};

    #[test]
    fn new_execution_policy_does_not_resume_previous_execution() {
        assert!(!should_resume_latest_target_role_thread(
            Some("new_execution"),
            crate::workflow::default_roles::CODER,
            false
        ));
    }

    #[test]
    fn resume_latest_target_role_thread_policy_resumes_previous_execution() {
        assert!(should_resume_latest_target_role_thread(
            Some(EXECUTION_POLICY_RESUME_LATEST_TARGET_ROLE_THREAD),
            crate::workflow::default_roles::CODER,
            false
        ));
    }

    #[test]
    fn review_rework_never_infers_a_coder_execution_or_harness_session() {
        assert!(!should_resume_latest_target_role_thread(
            Some(EXECUTION_POLICY_RESUME_LATEST_TARGET_ROLE_THREAD),
            crate::workflow::default_roles::CODER,
            true,
        ));
    }

    #[test]
    fn reviewer_never_infers_a_prior_execution_or_harness_session() {
        assert!(!should_resume_latest_target_role_thread(
            Some(EXECUTION_POLICY_RESUME_LATEST_TARGET_ROLE_THREAD),
            crate::workflow::default_roles::REVIEWER,
            false,
        ));
    }

    #[tokio::test]
    async fn load_sub_tasks_loads_complete_task_rows() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("pool creates");
        db::run_migrations(&pool).await.expect("migrations run");
        let db = db::SqliteDb::new(pool);

        let now = db::now_rfc3339();
        let project = ProjectRepo::create(
            &db,
            CreateProject {
                id: db::new_uuid_v4(),
                name: "dispatch context subtasks".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project creates");
        let parent = TaskRepo::create(
            &db,
            CreateTask {
                id: db::new_uuid_v4(),
                project_id: project.id.clone(),
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "parent".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "in_progress".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("parent task creates");
        let child = TaskRepo::create(
            &db,
            CreateTask {
                id: db::new_uuid_v4(),
                project_id: project.id,
                repo_id: None,
                parent_task_id: Some(parent.id.clone()),
                assignee_type: None,
                assignee_id: None,
                title: "child".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: Some(0),
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("child task creates");

        let loaded = load_sub_tasks(&db, &parent.id)
            .await
            .expect("subtasks load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, child.id);
        assert_eq!(loaded[0].task_type, "implementation");
        assert_eq!(
            loaded[0].parent_task_id.as_deref(),
            Some(parent.id.as_str())
        );
    }
}
