use super::*;

impl TaskService {
    /// Update a Task after validating any requested parent change against the
    /// Task's current Project and the supported one-level subtask shape.
    pub async fn update_task(&self, input: db::UpdateTask) -> Result<Task> {
        let task = TaskRepo::get_by_id(&*self.db, &input.id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", input.id.clone()))?;
        if task.version != input.expected_version {
            return Err(db::DbError::VersionConflict.into());
        }
        let parent_id = match input.parent_task_id.as_ref() {
            Some(Some(parent_id)) => Some(parent_id.as_str()),
            Some(None) => None,
            None => task.parent_task_id.as_deref(),
        };
        if let Some(parent_id) = parent_id {
            self.validate_task_parent_change(&input.id, parent_id)
                .await?;
        }
        TaskRepo::update(&*self.db, input).await.map_err(Into::into)
    }

    async fn validate_task_parent_change(&self, task_id: &str, parent_id: &str) -> Result<()> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        if parent_id == task_id {
            return Err(ServiceError::invalid_operation(
                "a Task cannot be its own parent",
            ));
        }

        let parent = TaskRepo::get_by_id(&*self.db, parent_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", parent_id.to_owned()))?;
        if parent.project_id != task.project_id {
            return Err(ServiceError::not_found("task", parent_id.to_owned()));
        }

        let mut seen = HashSet::new();
        let mut ancestor_id = parent_id.to_owned();
        loop {
            if ancestor_id == task_id {
                return Err(ServiceError::invalid_operation(
                    "Task parent change would create a cycle",
                ));
            }
            if !seen.insert(ancestor_id.clone()) {
                return Err(ServiceError::invalid_operation(
                    "Task parent chain already contains a cycle",
                ));
            }
            let ancestor = TaskRepo::get_by_id(&*self.db, &ancestor_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", ancestor_id.clone()))?;
            if ancestor.project_id != task.project_id {
                return Err(ServiceError::not_found("task", ancestor_id));
            }
            match ancestor.parent_task_id {
                Some(parent_id) => ancestor_id = parent_id,
                None => break,
            }
        }

        if parent.parent_task_id.is_some() {
            return Err(ServiceError::nested_subtask_unsupported());
        }
        Ok(())
    }
}
