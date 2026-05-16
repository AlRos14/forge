use super::*;

impl TaskService {
    pub async fn reorder_subtasks(
        &self,
        parent_task_id: String,
        ordered_ids: Vec<String>,
    ) -> Result<()> {
        validate_required("parent_task_id", &parent_task_id)?;
        let parent = TaskRepo::get_by_id(&*self.db, &parent_task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", parent_task_id.clone()))?;
        if !subtask::is_root_task(&self.db, &parent_task_id).await? {
            return Err(ServiceError::invalid_operation(format!(
                "task {parent_task_id} is not a root task"
            )));
        }

        let subtasks = TaskRepo::list_subtasks_ordered(&*self.db, &parent.id).await?;
        let subtask_ids: HashSet<_> = subtasks.iter().map(|s| s.id.clone()).collect();

        let submitted = ordered_ids.iter().cloned().collect::<HashSet<_>>();
        if submitted.len() != ordered_ids.len() {
            return Err(ServiceError::invalid_operation(
                "reorder payload must contain unique ids",
            ));
        }
        if submitted != subtask_ids {
            return Err(ServiceError::invalid_operation(
                "reorder payload must contain exactly the subtask ids",
            ));
        }

        TaskRepo::reorder_subtasks(&*self.db, &parent.id, &ordered_ids, &now_rfc3339()).await?;
        self.publish(ForgeEvent {
            event_type: "task.updated".to_owned(),
            entity_id: parent.id.clone(),
            timestamp: event_timestamp(),
            context: EventContext::TaskUpdated {
                project_id: parent.project_id,
            },
        });
        Ok(())
    }
}
