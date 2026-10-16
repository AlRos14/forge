use super::*;

pub struct NewSubtaskInput {
    pub title: String,
    pub description: Option<String>,
}

impl TaskService {
    pub async fn create_subtasks(
        &self,
        parent_task_id: String,
        items: Vec<NewSubtaskInput>,
    ) -> Result<Vec<Task>> {
        validate_required("parent_task_id", &parent_task_id)?;
        for item in &items {
            validate_required("title", &item.title)?;
        }

        // Take SQLite's writer reservation before reading the parent, so this
        // batch serializes with reparenting and single-child creation.
        let mut transaction = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let candidate =
            TaskRepo::get_by_id_in_tx(&*self.db, &mut transaction, &parent_task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", parent_task_id.clone()))?;
        let parent = super::subtask::validate_parent_placement_in_tx(
            &self.db,
            &mut transaction,
            None,
            &candidate.project_id,
            &parent_task_id,
        )
        .await?;
        let start_order =
            TaskRepo::next_subtask_order_in_tx(&*self.db, &mut transaction, &parent_task_id)
                .await?;

        let mut task_ids = Vec::with_capacity(items.len());
        for (offset, item) in items.into_iter().enumerate() {
            let task_id = new_uuid_v4();
            let now = now_rfc3339();
            let metadata = TaskMetadata {
                ..TaskMetadata::default()
            };
            let metadata_json = metadata.to_json();
            let task = TaskRepo::create_in_tx(
                &*self.db,
                &mut transaction,
                CreateTask {
                    id: task_id.clone(),
                    project_id: parent.project_id.clone(),
                    repo_id: parent.repo_id.clone(),
                    parent_task_id: Some(parent_task_id.clone()),
                    title: item.title.clone(),
                    description: item.description.clone(),
                    task_type: "implementation".to_owned(),
                    lifecycle_state: db::TaskLifecycleState::Ready,
                    is_automation: false,
                    priority: 0,
                    subtask_order: Some(start_order + offset as i64),
                    merge_config: None,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await?;
            TaskRepo::set_metadata_json_in_tx(
                &*self.db,
                &mut transaction,
                &task.id,
                metadata_json,
                &now,
            )
            .await?;
            task_ids.push(task_id);
        }
        transaction.commit().await?;

        let mut tasks = Vec::with_capacity(task_ids.len());
        for task_id in task_ids {
            let task = TaskRepo::get_by_id(&*self.db, &task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", task_id.clone()))?;
            self.publish(ForgeEvent {
                event_type: "task.created".to_owned(),
                entity_id: task.id.clone(),
                timestamp: event_timestamp(),
                context: EventContext::TaskCreated {
                    project_id: task.project_id.clone(),
                    title: task.title.clone(),
                },
            });
            tasks.push(task);
        }

        Ok(tasks)
    }
}
