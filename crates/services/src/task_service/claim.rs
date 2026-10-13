use super::*;

impl TaskService {
    /// Task-scoped claiming was the source of the one-Task/one-Workspace
    /// assumption. Callers must now allocate a WorkUnit and start its exact
    /// revision through `launch_work_unit_execution`.
    pub async fn claim_task(
        &self,
        task_id: impl Into<String>,
        _assignee: Assignee,
        _overrides: Option<ExecutionOverrides>,
    ) -> Result<ClaimedTask> {
        let task_id = task_id.into();
        validate_required("task_id", &task_id)?;
        Err(ServiceError::invalid_operation(
            "Task-scoped claim is retired; allocate and start an exact WorkUnit",
        ))
    }
}
