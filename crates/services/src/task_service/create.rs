use super::*;
use api_types::ActorRef;

impl TaskService {
    #[allow(clippy::too_many_arguments)]
    pub async fn create_task(
        &self,
        project_id: impl Into<String>,
        title: impl Into<String>,
        description: Option<String>,
        parent_task_id: Option<String>,
        priority: Option<i64>,
        task_type: Option<String>,
        task_state_config: Option<String>,
        merge_config: Option<Value>,
        role_assignments: Option<Vec<api_types::InitialRoleAssignment>>,
    ) -> Result<Task> {
        self.create_task_inner(
            project_id,
            title,
            description,
            parent_task_id,
            priority,
            task_type,
            task_state_config,
            merge_config,
            role_assignments,
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn create_task_inner(
        &self,
        project_id: impl Into<String>,
        title: impl Into<String>,
        description: Option<String>,
        parent_task_id: Option<String>,
        priority: Option<i64>,
        task_type: Option<String>,
        task_state_config: Option<String>,
        merge_config: Option<Value>,
        role_assignments: Option<Vec<api_types::InitialRoleAssignment>>,
    ) -> Result<Task> {
        let project_id = project_id.into();
        let title = title.into();
        validate_required("project_id", &project_id)?;
        validate_required("title", &title)?;

        let project = ProjectRepo::get_by_id(&*self.db, &project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.clone()))?;
        let (repo_id, subtask_order) = if let Some(parent_id) = parent_task_id.as_deref() {
            validate_required("parent_task_id", parent_id)?;
            let parent = TaskRepo::get_by_id(&*self.db, parent_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", parent_id.to_owned()))?;
            if parent.parent_task_id.is_some() {
                return Err(ServiceError::nested_subtask_unsupported());
            }
            (
                parent.repo_id,
                Some(TaskRepo::next_subtask_order(&*self.db, parent_id).await?),
            )
        } else {
            (project.primary_repo_id.clone(), None)
        };

        let is_subtask = parent_task_id.is_some();
        let effective_task_type = task_type.unwrap_or_else(|| "implementation".to_owned());
        if !matches!(
            effective_task_type.as_str(),
            "implementation" | "planning" | "discovery" | "review" | "validation"
        ) {
            return Err(ServiceError::invalid_operation(
                "task_type must be implementation, planning, discovery, review, or validation",
            ));
        }
        let now = now_rfc3339();
        let no_repo = repo_id.is_none();
        // New Task progress starts in the aggregate lifecycle directly. A
        // project workflow state name must not decide whether the Task is
        // ready, active, or blocked.
        let initial_status = if no_repo { "backlog" } else { "todo" }.to_owned();
        let validated_assignments = if let Some(ref assignments) = role_assignments {
            let mut validated = Vec::with_capacity(assignments.len());
            for assignment in assignments {
                if db::canonical_task_role_name(&assignment.role_name).is_none() {
                    return Err(ServiceError::invalid_operation(format!(
                        "role is not a TaskRole: {}",
                        assignment.role_name
                    )));
                }
                let assignee_type: AssigneeKind = match assignment.assignee_type {
                    api_types::assignee::AssigneeKind::Agent => AssigneeKind::Agent,
                    api_types::assignee::AssigneeKind::User => AssigneeKind::User,
                };
                let assignee_id = assignment
                    .assignee_id
                    .clone()
                    .filter(|id| !id.trim().is_empty())
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "role assignment for '{}' requires assignee_id",
                            assignment.role_name
                        ))
                    })?;
                validated.push((assignment.role_name.clone(), assignee_type, assignee_id));
            }
            Some(validated)
        } else {
            None
        };
        if let Some(assignments) = validated_assignments.as_deref() {
            for (_, assignee_type, assignee_id) in assignments {
                if let Some(actor_ref) = actor_ref_for_assignment(assignee_type, assignee_id) {
                    self.validate_actor_for_project(&project, &actor_ref)
                        .await?;
                }
            }
        }
        let metadata_json = if is_subtask {
            let metadata = TaskMetadata {
                ..TaskMetadata::default()
            };
            metadata.to_json()
        } else {
            None
        };
        let create_task = CreateTask {
            id: new_uuid_v4(),
            project_id,
            repo_id,
            parent_task_id,
            subtask_order,
            assignee_type: None,
            assignee_id: None,
            title,
            description,
            task_type: effective_task_type,
            status: initial_status,
            is_automation: false,
            priority: priority.unwrap_or(0),
            task_state_config,
            merge_config: serialize_config(merge_config)?,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let mut transaction = self.db.pool().begin().await?;
        let mut task = TaskRepo::create_in_tx(&*self.db, &mut transaction, create_task).await?;
        if !task.is_automation {
            ProjectRepo::increment_project_work_epoch(
                &*self.db,
                &mut transaction,
                &task.project_id,
                1,
            )
            .await?;
        }
        transaction.commit().await?;
        if is_subtask {
            TaskRepo::set_metadata_json(&*self.db, &task.id, metadata_json.clone(), &now).await?;
            task.metadata_json = metadata_json;
        }

        if let Some(assignments) = validated_assignments {
            for (role_name, assignee_type, assignee_id) in assignments {
                self.assign_role_membership(CreateTaskRoleAssignment {
                    id: new_uuid_v4(),
                    task_id: task.id.clone(),
                    role_name,
                    assignee_type: Some(assignee_type),
                    assignee_id: Some(assignee_id),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                })
                .await?;
            }
        }

        self.publish(ForgeEvent {
            event_type: "task.created".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskCreated {
                project_id: task.project_id.clone(),
                title: task.title.clone(),
            },
        });

        Ok(task)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn create_automation_task(
        &self,
        project_id: impl Into<String>,
        title: impl Into<String>,
        description: Option<String>,
        task_type: Option<String>,
        task_state_config: Option<String>,
        merge_config: Option<Value>,
    ) -> Result<Task> {
        let project_id = project_id.into();
        let title = title.into();
        validate_required("project_id", &project_id)?;
        validate_required("title", &title)?;

        let project = ProjectRepo::get_by_id(&*self.db, &project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.clone()))?;
        let repo_id = project.primary_repo_id.clone();
        let initial_status = if repo_id.is_none() { "backlog" } else { "todo" }.to_owned();

        let now = now_rfc3339();
        let effective_task_type = task_type.unwrap_or_else(|| "implementation".to_owned());
        let create_task = CreateTask {
            id: new_uuid_v4(),
            project_id,
            repo_id,
            parent_task_id: None,
            subtask_order: None,
            assignee_type: None,
            assignee_id: None,
            title,
            description,
            task_type: effective_task_type,
            status: initial_status,
            is_automation: true,
            priority: 0,
            task_state_config,
            merge_config: serialize_config(merge_config)?,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let mut transaction = self.db.pool().begin().await?;
        let task = TaskRepo::create_in_tx(&*self.db, &mut transaction, create_task).await?;
        transaction.commit().await?;

        self.publish(ForgeEvent {
            event_type: "task.created".to_owned(),
            entity_id: task.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskCreated {
                project_id: task.project_id.clone(),
                title: task.title.clone(),
            },
        });

        Ok(task)
    }

    pub async fn duplicate_task(&self, source_task_id: &str) -> Result<Task> {
        let source = TaskRepo::get_by_id(&*self.db, source_task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", source_task_id.to_owned()))?;
        let task = self
            .create_task(
                source.project_id,
                source.title,
                source.description,
                None,
                Some(source.priority),
                Some(source.task_type),
                source.task_state_config,
                source
                    .merge_config
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
                None,
            )
            .await?;
        TaskRepo::get_by_id(&*self.db, &task.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task.id))
    }
}

fn actor_ref_for_assignment(assignee_type: &AssigneeKind, assignee_id: &str) -> Option<ActorRef> {
    match assignee_type {
        AssigneeKind::Agent => Some(ActorRef::Agent(assignee_id.to_owned())),
        AssigneeKind::User if assignee_id != "human" => {
            Some(ActorRef::Human(assignee_id.to_owned()))
        }
        // The pre-existing "human" project-settings sentinel is a bounded
        // legacy/manual path, not a persisted ActorRef.
        AssigneeKind::User => None,
    }
}
