use super::*;

impl TaskService {
    pub async fn dispatch_role_follow_up(
        &self,
        task_id: &str,
        role: &str,
        parent_execution_id: String,
        prompt: String,
        trigger: &str,
        purpose: ExecutionPurpose,
        idempotency_key: String,
    ) -> Result<Execution> {
        self.dispatch_role_follow_up_exact(
            task_id,
            role,
            parent_execution_id,
            None,
            prompt,
            trigger,
            purpose,
            idempotency_key,
        )
        .await
    }

    pub async fn dispatch_role_follow_up_with_agent(
        &self,
        task_id: &str,
        role: &str,
        parent_execution_id: String,
        agent_id: String,
        prompt: String,
        trigger: &str,
        purpose: ExecutionPurpose,
        idempotency_key: String,
    ) -> Result<Execution> {
        self.dispatch_role_follow_up_exact(
            task_id,
            role,
            parent_execution_id,
            Some(agent_id),
            prompt,
            trigger,
            purpose,
            idempotency_key,
        )
        .await
    }

    async fn dispatch_role_follow_up_exact(
        &self,
        task_id: &str,
        role: &str,
        parent_execution_id: String,
        agent_id: Option<String>,
        prompt: String,
        trigger: &str,
        purpose: ExecutionPurpose,
        idempotency_key: String,
    ) -> Result<Execution> {
        validate_required("task_id", task_id)?;
        validate_required("role", role)?;
        validate_required("parent_execution_id", &parent_execution_id)?;
        let parent = ExecutionRepo::get_by_id(&*self.db, &parent_execution_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", parent_execution_id.clone()))?;
        if parent.task_id != task_id
            || parent.role != role
            || parent
                .purpose
                .as_ref()
                .is_some_and(|parent_purpose| parent_purpose != &purpose)
            || parent.work_unit_id.is_none()
        {
            return Err(ServiceError::invalid_operation(
                "role follow-up requires the exact parent Execution, matching role and purpose, bound to a WorkUnit",
            ));
        }
        let result = self
            .follow_up_execution(parent.id.clone(), prompt, agent_id, None, idempotency_key)
            .await?;
        if !result.idempotent_replay {
            self.start_execution(result.execution.id.clone()).await?;
            self.publish(ForgeEvent {
                event_type: "follow_up.dispatched".to_owned(),
                entity_id: task_id.to_owned(),
                timestamp: event_timestamp(),
                context: EventContext::FollowUpDispatched {
                    task_id: task_id.to_owned(),
                    parent_execution_id: parent.id,
                    execution_id: result.execution.id.clone(),
                    trigger: trigger.to_owned(),
                },
            });
        }
        ExecutionRepo::get_by_id(&*self.db, &result.execution.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("execution", result.execution.id))
    }
}
