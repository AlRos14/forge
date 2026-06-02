use super::*;

impl TaskService {
    pub async fn dispatch_initial_role_execution(
        &self,
        task_id: &str,
        agent_id: &str,
        role: &str,
        prompt: String,
    ) -> Result<Execution> {
        self.dispatch_initial_role_execution_with_metadata(task_id, agent_id, role, prompt, None)
            .await
    }

    pub async fn dispatch_initial_role_execution_with_metadata(
        &self,
        task_id: &str,
        agent_id: &str,
        role: &str,
        prompt: String,
        dispatch_metadata: Option<Value>,
    ) -> Result<Execution> {
        validate_required("task_id", task_id)?;
        validate_required("agent_id", agent_id)?;
        validate_required("role", role)?;

        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        self.check_dependency_gate(&task, agent_id).await?;
        let agent = AgentRepo::get_by_id(&*self.db, agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.to_owned()))?;
        let workspace = prepare_workspace(
            &self.db,
            &self.workspace_root,
            &task,
            &task.id,
            self.repo_cache_locks.clone(),
        )
        .await?;
        let executor_config_snapshot_json = with_dispatch_metadata(
            build_executor_config_snapshot(&self.db, &task, &agent, None).await?,
            dispatch_metadata,
        )?;
        let now = now_rfc3339();
        let execution = ExecutionRepo::create(
            &*self.db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                agent_id: Some(agent.id.clone()),
                role: role.to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(prompt),
                logs_path: None,
                before_sha: workspace.before_sha.clone(),
                after_sha: None,
                error: None,
                executor_config_snapshot_json,
                workspace_id: Some(workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %agent.id,
            role = %role,
            execution_id = %execution.id,
            "initial role execution dispatched"
        );

        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id: agent.id.clone(),
                execution_id: execution.id.clone(),
            },
        });

        self.start_execution(execution.id.clone()).await?;

        Ok(execution)
    }

    pub async fn launch_execution(
        &self,
        task_id: impl Into<String>,
        agent_id: impl Into<String>,
        summary: Option<String>,
        overrides: Option<ExecutionOverrides>,
    ) -> Result<LaunchExecutionResult> {
        let task_id = task_id.into();
        let agent_id = agent_id.into();
        validate_required("task_id", &task_id)?;
        validate_required("agent_id", &agent_id)?;

        let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for(&task, &project.workflow_definition);
        if workflow.state_kind(&task.status) == Some(api_types::StateKind::Terminal) {
            return Err(ServiceError::invalid_operation(format!(
                "cannot launch execution for task {} in terminal status {}",
                task.id, task.status
            )));
        }
        self.check_dependency_gate(&task, &agent_id).await?;
        self.ensure_no_running_interactive_execution(&task.id)
            .await?;

        let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;
        let workspace = prepare_workspace(
            &self.db,
            &self.workspace_root,
            &task,
            &task_id,
            self.repo_cache_locks.clone(),
        )
        .await?;
        self.run_blocking_before_work_preflight(&task, &project, &workspace, Some(&agent_id), None)
            .await?;
        let executor_config_snapshot_json =
            build_executor_config_snapshot(&self.db, &task, &agent, overrides).await?;
        let now = now_rfc3339();
        let execution = ExecutionRepo::create(
            &*self.db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                agent_id: Some(agent.id.clone()),
                role: "interactive".to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary,
                logs_path: None,
                before_sha: workspace.before_sha.clone(),
                after_sha: None,
                error: None,
                executor_config_snapshot_json,
                workspace_id: Some(workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %agent.id,
            execution_id = %execution.id,
            role = "interactive",
            "execution launched"
        );

        let task = {
            let kind = workflow.state_kind(&task.status);
            if matches!(
                kind,
                Some(api_types::StateKind::Initial | api_types::StateKind::Custom)
            ) {
                let target = first_launch_target(&workflow, &task.status);
                if let Some(target) = target {
                    self.transition(task.id.clone(), target, task.version)
                        .await?
                        .task
                } else {
                    task
                }
            } else {
                task
            }
        };

        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id,
                execution_id: execution.id.clone(),
            },
        });

        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
        })
    }

    pub async fn follow_up_execution(
        &self,
        parent_execution_id: impl Into<String>,
        message: String,
        agent_id: Option<String>,
        overrides: Option<ExecutionOverrides>,
    ) -> Result<LaunchExecutionResult> {
        let parent_execution_id = parent_execution_id.into();
        validate_required("parent_execution_id", &parent_execution_id)?;

        let parent_execution = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        if !matches!(
            parent_execution.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "follow-up requires a completed, failed, or cancelled execution, got {}",
                parent_execution.status
            )));
        }
        let parent_agent_session_id =
            parent_execution.agent_session_id.clone().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "parent execution has no resumable session (agent_session_id is null)",
                )
            })?;

        let task = TaskRepo::get_by_id(&*self.db, &parent_execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", parent_execution.task_id.clone()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for(&task, &project.workflow_definition);
        if workflow.state_kind(&task.status) == Some(api_types::StateKind::Terminal) {
            return Err(ServiceError::invalid_operation(format!(
                "cannot follow up on a task in terminal status {}",
                task.status
            )));
        }

        let resolved_agent_id = agent_id
            .or_else(|| parent_execution.agent_id.clone())
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "follow-up requires agent_id either in request or parent execution",
                )
            })?;
        let agent = AgentRepo::get_by_id(&*self.db, &resolved_agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", resolved_agent_id.clone()))?;

        let parent_executor_type = parent_execution
            .executor_config_snapshot_json
            .as_deref()
            .ok_or_else(|| {
                ServiceError::invalid_operation("parent execution missing executor config snapshot")
            })
            .and_then(|snapshot_json| {
                serde_json::from_str::<Value>(snapshot_json).map_err(|error| {
                    ServiceError::invalid_operation(format!(
                        "invalid parent executor config snapshot: {error}"
                    ))
                })
            })?
            .get("executor_type")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                ServiceError::invalid_operation("parent execution snapshot missing executor_type")
            })?
            .to_owned();
        if parent_executor_type != agent.executor_type {
            return Err(ServiceError::invalid_operation(format!(
                "follow-up requires same executor type: parent used '{}' but agent '{}' uses '{}'",
                parent_executor_type, agent.id, agent.executor_type
            )));
        }

        self.ensure_no_running_interactive_execution(&task.id)
            .await?;
        let workspace = prepare_workspace(
            &self.db,
            &self.workspace_root,
            &task,
            &task.id,
            self.repo_cache_locks.clone(),
        )
        .await?;
        let mut executor_config_snapshot_json =
            build_executor_config_snapshot(&self.db, &task, &agent, overrides).await?;
        if let Some(snapshot_json) = executor_config_snapshot_json.as_deref() {
            executor_config_snapshot_json = Some(executor_snapshot_with_resume_thread(
                snapshot_json,
                &parent_agent_session_id,
            )?);
        }

        let now = now_rfc3339();
        let execution = ExecutionRepo::create(
            &*self.db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                agent_id: Some(resolved_agent_id.clone()),
                role: "interactive".to_owned(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: Some(parent_execution_id),
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(message),
                logs_path: None,
                before_sha: workspace.before_sha.clone(),
                after_sha: None,
                error: None,
                executor_config_snapshot_json,
                workspace_id: Some(workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %resolved_agent_id,
            execution_id = %execution.id,
            parent_execution_id = %parent_execution.id,
            role = "interactive",
            "follow-up execution launched"
        );

        let task = {
            let kind = workflow.state_kind(&task.status);
            if matches!(
                kind,
                Some(api_types::StateKind::Initial | api_types::StateKind::Custom)
            ) {
                let target = first_launch_target(&workflow, &task.status);
                if let Some(target) = target {
                    self.transition(task.id.clone(), target, task.version)
                        .await?
                        .task
                } else {
                    task
                }
            } else {
                task
            }
        };

        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id: resolved_agent_id,
                execution_id: execution.id.clone(),
            },
        });

        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
        })
    }

    pub async fn re_execute_execution(
        &self,
        parent_execution_id: impl Into<String>,
    ) -> Result<LaunchExecutionResult> {
        self.re_execute_execution_with_context(parent_execution_id, None)
            .await
    }

    pub async fn re_execute_execution_with_context(
        &self,
        parent_execution_id: impl Into<String>,
        context: Option<String>,
    ) -> Result<LaunchExecutionResult> {
        let parent_execution_id = parent_execution_id.into();
        validate_required("parent_execution_id", &parent_execution_id)?;

        let parent_execution = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        if !matches!(
            parent_execution.status,
            ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Cancelled
        ) {
            return Err(ServiceError::invalid_operation(format!(
                "re-execute requires a completed, failed, or cancelled execution, got {}",
                parent_execution.status
            )));
        }

        let task = TaskRepo::get_by_id(&*self.db, &parent_execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", parent_execution.task_id.clone()))?;
        let page = ExecutionRepo::list_by_task(
            &*self.db,
            &task.id,
            PageRequest {
                cursor: None,
                limit: 20,
                include_total: false,
                sort_by: SortBy::CreatedAt,
                sort_order: SortOrder::Desc,
            },
        )
        .await?;
        if page.items.into_iter().any(|execution| {
            execution.status == ExecutionStatus::Running && execution.role == parent_execution.role
        }) {
            return Err(ServiceError::invalid_operation(format!(
                "execution already running for role {}",
                parent_execution.role
            )));
        }

        let agent_id = parent_execution.agent_id.clone().ok_or_else(|| {
            ServiceError::invalid_operation(format!(
                "parent execution {} missing agent_id",
                parent_execution.id
            ))
        })?;
        let agent = AgentRepo::get_by_id(&*self.db, &agent_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", agent_id.clone()))?;

        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let workflow = WorkflowEngine::resolve_workflow_for(&task, &project.workflow_definition);
        // Verify the execution role matches the current state effective role for cascade eligibility
        let current_state = workflow.states.iter().find(|s| s.name == task.status);
        let effective_role = current_state.and_then(|s| {
            s.role.clone().or_else(|| {
                if s.kind == api_types::StateKind::Active {
                    Some("assignee".to_owned())
                } else {
                    None
                }
            })
        });
        if effective_role.as_deref() != Some(&parent_execution.role)
            && parent_execution.role != "interactive"
        {
            tracing::info!(
                task_id = %task.id,
                execution_role = %parent_execution.role,
                effective_role = ?effective_role,
                "re-execute role does not match current state effective role; execution will not cascade"
            );
        }
        if workflow.state_kind(&task.status) == Some(api_types::StateKind::Terminal) {
            return Err(ServiceError::invalid_operation(format!(
                "cannot re-execute a task in terminal status {}",
                task.status
            )));
        }

        let workspace = prepare_workspace(
            &self.db,
            &self.workspace_root,
            &task,
            &task.id,
            self.repo_cache_locks.clone(),
        )
        .await?;
        let executor_config_snapshot_json =
            build_executor_config_snapshot(&self.db, &task, &agent, None).await?;
        let role_name = &parent_execution.role;
        let state = workflow
            .states
            .iter()
            .find(|state| state.name == task.status);
        let state_config = state
            .map(|state| state.config.clone())
            .unwrap_or_else(|| serde_json::Value::Object(Default::default()));
        let state_dispatch =
            dispatch_intent_from_workflow_dispatch(state.and_then(|state| state.dispatch.as_ref()));
        let selection = effective_prompt_selection(role_name, None, state_dispatch.as_ref());
        let dispatch_ctx = load_agent_dispatch_context(
            Arc::clone(&self.db),
            &task.id,
            role_name,
            &task.status,
            state_config,
            Some(selection.execution_policy.as_str()),
            &workflow,
        )
        .await?;
        let (prompt, _selection) =
            build_effective_prompt(&dispatch_ctx, None, state_dispatch.as_ref());
        let summary = match context {
            Some(ctx) => format!("[User context: {ctx}]\n\n{}", prompt.user),
            None => prompt.user,
        };
        let now = now_rfc3339();
        let execution = ExecutionRepo::create(
            &*self.db,
            CreateExecution {
                id: new_uuid_v4(),
                task_id: task.id.clone(),
                agent_id: Some(agent.id.clone()),
                role: parent_execution.role.clone(),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(summary),
                logs_path: None,
                before_sha: workspace.before_sha.clone(),
                after_sha: None,
                error: None,
                executor_config_snapshot_json,
                workspace_id: Some(workspace.id.clone()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await?;

        tracing::info!(
            task_id = %task.id,
            agent_id = %agent.id,
            execution_id = %execution.id,
            parent_execution_id = %parent_execution.id,
            role = %execution.role,
            "re-execute execution launched"
        );

        let task = {
            let kind = workflow.state_kind(&task.status);
            if matches!(
                kind,
                Some(api_types::StateKind::Initial | api_types::StateKind::Custom)
            ) {
                let target = first_launch_target(&workflow, &task.status);
                if let Some(target) = target {
                    self.transition(task.id.clone(), target, task.version)
                        .await?
                        .task
                } else {
                    task
                }
            } else {
                task
            }
        };

        self.publish(ForgeEvent {
            event_type: "task.execution_launched".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskAssigned {
                project_id: task.project_id.clone(),
                agent_id,
                execution_id: execution.id.clone(),
            },
        });

        Ok(LaunchExecutionResult {
            task,
            execution,
            workspace,
        })
    }

    pub async fn cancel_execution(
        &self,
        execution_id: impl Into<String>,
        reason: String,
    ) -> Result<Execution> {
        let execution_id = execution_id.into();
        let execution = ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.clone()))?;
        if execution.status != ExecutionStatus::Running {
            return Err(ServiceError::invalid_operation(format!(
                "can only cancel a running execution, got {}",
                execution.status
            )));
        }
        let now = now_rfc3339();
        self.cancel_active_execution(
            &execution,
            &reason,
            db::StopReason::UserCancelled,
            "user:api",
            db::ResumePolicy::Manual,
        )
        .await?;
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let mut recovery_actions = vec![
            api_types::RecoveryAction::Reexecute,
            api_types::RecoveryAction::ResetToInitial,
            api_types::RecoveryAction::CancelTask,
        ];
        if execution.agent_session_id.is_some() {
            recovery_actions.insert(0, api_types::RecoveryAction::ResumeSession);
        }
        let annotation = api_types::TaskBlockingAnnotation {
            annotation_type: "manual_stop".to_owned(),
            blocking_reason: "user_cancelled".to_owned(),
            blocked_by: Some("user:api".to_owned()),
            blocked_at: Some(now.clone()),
            blocked_execution_id: Some(execution.id.clone()),
            artifact: Some(api_types::BlockingArtifact {
                kind: "execution".to_owned(),
                id: Some(execution.id.clone()),
                log_path: None,
            }),
            message: Some("Execution stopped by user".to_owned()),
            hook: None,
            recovery_actions,
        };
        let annotation = serde_json::to_string(&annotation).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "failed to serialize manual-stop annotation: {error}"
            ))
        })?;
        let _ = TaskRepo::update(
            &*self.db,
            UpdateTask {
                id: task.id.clone(),
                expected_version: task.version,
                title: None,
                description: None,
                priority: None,
                merge_config: None,
                plan: None,
                error_annotation: Some(Some(annotation)),
                blocked_json: None,
                failed_json: None,
                task_state_config: None,
                parent_task_id: None,
                updated_at: now.clone(),
            },
        )
        .await?;
        ExecutionRepo::get_by_id(&*self.db, &execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", execution_id))
    }
}

fn with_dispatch_metadata(
    snapshot_json: Option<String>,
    dispatch_metadata: Option<Value>,
) -> Result<Option<String>> {
    let Some(snapshot_json) = snapshot_json else {
        return Ok(None);
    };
    let Some(dispatch_metadata) = dispatch_metadata else {
        return Ok(Some(snapshot_json));
    };
    let mut snapshot = serde_json::from_str::<Value>(&snapshot_json).map_err(|error| {
        ServiceError::invalid_operation(format!("invalid executor config snapshot: {error}"))
    })?;
    let Some(snapshot_obj) = snapshot.as_object_mut() else {
        return Ok(Some(snapshot_json));
    };
    snapshot_obj.insert("dispatch".to_string(), dispatch_metadata);
    serde_json::to_string(&snapshot)
        .map(Some)
        .map_err(|error| ServiceError::invalid_operation(format!("invalid JSON snapshot: {error}")))
}

fn first_launch_target(workflow: &api_types::WorkflowDefinition, from: &str) -> Option<String> {
    let source_kind = workflow.state_kind(from);
    let targets = workflow
        .outgoing_trigger_targets(from)
        .filter(|(trigger, _)| {
            !trigger.system_only()
                || matches!(
                    source_kind,
                    Some(api_types::StateKind::Initial | api_types::StateKind::Custom)
                )
        })
        .filter_map(|(_, target)| workflow.state_kind(&target).map(|kind| (target, kind)))
        .collect::<Vec<_>>();

    targets
        .iter()
        .find(|(_, kind)| *kind == api_types::StateKind::Active)
        .or_else(|| {
            targets
                .iter()
                .find(|(_, kind)| *kind == api_types::StateKind::Gate)
        })
        .map(|(target, _)| target.clone())
}
