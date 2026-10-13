use super::*;

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
        initial_lifecycle_state: Option<db::TaskLifecycleState>,
        merge_config: Option<Value>,
    ) -> Result<Task> {
        self.create_task_inner(
            project_id,
            title,
            description,
            parent_task_id,
            priority,
            task_type,
            initial_lifecycle_state,
            merge_config,
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
        initial_lifecycle_state: Option<db::TaskLifecycleState>,
        merge_config: Option<Value>,
    ) -> Result<Task> {
        let project_id = project_id.into();
        let title = title.into();
        validate_required("project_id", &project_id)?;
        validate_required("title", &title)?;

        let project = ProjectRepo::get_by_id(&*self.db, &project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", project_id.clone()))?;
        let is_subtask = parent_task_id.is_some();
        if let Some(parent_id) = parent_task_id.as_deref() {
            validate_required("parent_task_id", parent_id)?;
        }
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
        let metadata_json = if is_subtask {
            let metadata = TaskMetadata {
                ..TaskMetadata::default()
            };
            metadata.to_json()
        } else {
            None
        };
        // BEGIN IMMEDIATE takes SQLite's writer reservation before the parent
        // and sibling reads. Competing creates/reparents therefore validate
        // against one serial structural state.
        let mut transaction = if is_subtask {
            self.db.pool().begin_with("BEGIN IMMEDIATE").await?
        } else {
            self.db.pool().begin().await?
        };
        let (repo_id, subtask_order) = if let Some(parent_id) = parent_task_id.as_deref() {
            let parent = super::subtask::validate_parent_placement_in_tx(
                &self.db,
                &mut transaction,
                None,
                &project_id,
                parent_id,
            )
            .await?;
            let order =
                TaskRepo::next_subtask_order_in_tx(&*self.db, &mut transaction, parent_id).await?;
            (parent.repo_id, Some(order))
        } else {
            (project.primary_repo_id.clone(), None)
        };
        let no_repo = repo_id.is_none();
        // New Task progress starts in the aggregate lifecycle directly. A
        // project workflow state name must not decide whether the Task is
        // ready, active, or blocked.
        let lifecycle_state = initial_lifecycle_state.unwrap_or(if no_repo {
            db::TaskLifecycleState::Backlog
        } else {
            db::TaskLifecycleState::Ready
        });
        let create_task = CreateTask {
            id: new_uuid_v4(),
            project_id,
            repo_id,
            parent_task_id,
            subtask_order,
            title,
            description,
            task_type: effective_task_type,
            lifecycle_state,
            is_automation: false,
            priority: priority.unwrap_or(0),
            merge_config: serialize_config(merge_config)?,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let mut task = TaskRepo::create_in_tx(&*self.db, &mut transaction, create_task).await?;
        if is_subtask {
            TaskRepo::set_metadata_json_in_tx(
                &*self.db,
                &mut transaction,
                &task.id,
                metadata_json.clone(),
                &now,
            )
            .await?;
        }
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
            task.metadata_json = metadata_json;
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
        initial_lifecycle_state: Option<db::TaskLifecycleState>,
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
        let lifecycle_state = initial_lifecycle_state.unwrap_or(if repo_id.is_none() {
            db::TaskLifecycleState::Backlog
        } else {
            db::TaskLifecycleState::Ready
        });

        let now = now_rfc3339();
        let effective_task_type = task_type.unwrap_or_else(|| "implementation".to_owned());
        let create_task = CreateTask {
            id: new_uuid_v4(),
            project_id,
            repo_id,
            parent_task_id: None,
            subtask_order: None,
            title,
            description,
            task_type: effective_task_type,
            lifecycle_state,
            is_automation: true,
            priority: 0,
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
                None,
                source
                    .merge_config
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok()),
            )
            .await?;
        TaskRepo::get_by_id(&*self.db, &task.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task.id))
    }
}
