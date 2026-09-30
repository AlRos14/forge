use super::*;
use crate::orchestrator_runtime::TaskRoleOrchestratorPolicy;
use db::{
    ActorKind, ActorRef, CreateDomainEvent, CreateExecution, DomainEventRepo, OrchestratorWake,
    OrchestratorWakeExecution, OrchestratorWakeRepo, RoleMembershipRepo, RoleMembershipStatus,
    TaskRoleRepo,
};

impl TaskService {
    pub(crate) async fn validate_orchestrator_execution(
        &self,
        execution: &Execution,
    ) -> Result<Option<OrchestratorWake>> {
        if execution.role != "orchestrator"
            || execution.purpose != Some(ExecutionPurpose::Orchestrate)
            || execution.workspace_id.is_some()
        {
            return Ok(None);
        }
        let Some((wake, attempt)) =
            OrchestratorWakeRepo::get_orchestrator_wake_by_execution(&*self.db, &execution.id)
                .await?
        else {
            return Err(ServiceError::invalid_operation(
                "workspace-free orchestrator Execution has no durable wake authority",
            ));
        };
        if wake.task_id != execution.task_id
            || wake.actor_kind != ActorKind::Agent
            || execution.actor_ref() != Some(ActorRef::Agent(wake.actor_id.clone()))
            || wake.current_attempt != Some(attempt.attempt_number)
            || !matches!(
                wake.state,
                db::OrchestratorWakeState::Leased | db::OrchestratorWakeState::Running
            )
            || !matches!(
                attempt.state.as_str(),
                "reserved" | "start_requested" | "running"
            )
        {
            return Err(ServiceError::invalid_operation(
                "orchestrator Execution does not match its durable wake and current attempt",
            ));
        }
        let role = TaskRoleRepo::get_by_id(&*self.db, &wake.task_role_id)
            .await?
            .ok_or_else(|| ServiceError::invalid_operation("orchestrator TaskRole is missing"))?;
        if role.task_id != execution.task_id
            || role.role != "orchestrator"
            || role.coordination_mode != wake.coordination_mode
            || role.version != wake.task_role_version
            || role.policy_json != wake.task_role_policy_json
        {
            return Err(ServiceError::invalid_operation(
                "orchestrator Execution TaskRole version or policy binding is invalid",
            ));
        }
        let policy = TaskRoleOrchestratorPolicy::parse(&role.policy_json)
            .map_err(ServiceError::invalid_operation)?;
        if !policy.permits_automatic_orchestration() {
            return Err(ServiceError::invalid_operation(
                "TaskRole policy disables automatic Agent orchestration",
            ));
        }
        let memberships = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
        if !memberships.iter().any(|membership| {
            membership.status == RoleMembershipStatus::Active
                && membership.actor_kind == ActorKind::Agent
                && membership.actor_id == wake.actor_id
        }) {
            return Err(ServiceError::invalid_operation(
                "orchestrator Agent is no longer an active TaskRole member",
            ));
        }
        Ok(Some(wake))
    }

    pub(crate) async fn dispatch_orchestrator_execution(
        &self,
        wake: &OrchestratorWake,
        attempt: &OrchestratorWakeExecution,
        lease_owner: &str,
        prompt: String,
    ) -> Result<Execution> {
        if wake.state != db::OrchestratorWakeState::Leased
            || wake.lease_owner.as_deref() != Some(lease_owner)
            || attempt.wake_id != wake.id
            || attempt.state != "reserved"
            || wake.current_attempt != Some(attempt.attempt_number)
        {
            return Err(ServiceError::invalid_operation(
                "orchestrator wake lease or reserved Execution attempt is stale",
            ));
        }
        let task = TaskRepo::get_by_id(&*self.db, &wake.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", wake.task_id.clone()))?;
        let task_role = TaskRoleRepo::get_by_id(&*self.db, &wake.task_role_id)
            .await?
            .ok_or_else(|| ServiceError::invalid_operation("orchestrator TaskRole is missing"))?;
        if task_role.task_id != task.id
            || task_role.role != "orchestrator"
            || task_role.coordination_mode != wake.coordination_mode
            || task_role.version != wake.task_role_version
            || task_role.policy_json != wake.task_role_policy_json
        {
            return Err(ServiceError::invalid_operation(
                "orchestrator wake no longer resolves to its exact TaskRole policy snapshot",
            ));
        }
        let policy = TaskRoleOrchestratorPolicy::parse(&task_role.policy_json)
            .map_err(ServiceError::invalid_operation)?;
        if !policy.permits_automatic_orchestration() {
            return Err(ServiceError::invalid_operation(
                "TaskRole policy disables automatic Agent orchestration",
            ));
        }
        let memberships = RoleMembershipRepo::list_by_role(&*self.db, &task_role.id, false).await?;
        if !memberships.iter().any(|membership| {
            membership.status == RoleMembershipStatus::Active
                && membership.actor_kind == wake.actor_kind
                && membership.actor_id == wake.actor_id
        }) {
            return Err(ServiceError::invalid_operation(
                "orchestrator Actor is no longer an active member",
            ));
        }
        if wake.actor_kind != ActorKind::Agent {
            return Err(ServiceError::invalid_operation(
                "Human orchestrator wake cannot be sent to a Harness",
            ));
        }
        let agent = AgentRepo::get_by_id(&*self.db, &wake.actor_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", wake.actor_id.clone()))?;
        // The current safe automatic profile is a Codex CLI with an explicit
        // read-only sandbox. Other adapters stay durable and fail closed until
        // they expose an equally strong, verified sandbox capability.
        if agent.backend_kind == "native" || agent.executor_type != "codex" {
            return Err(ServiceError::invalid_operation(
                "PR6 automatic orchestration requires a Codex CLI Agent with read-only sandbox support",
            ));
        }

        let mut snapshot = build_executor_config_snapshot(
            &self.db,
            &task,
            &agent,
            None,
            self.adapter_registry.as_deref(),
        )
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("Agent has no executable config snapshot")
        })?;
        let mut snapshot_value: Value = serde_json::from_str(&snapshot)
            .map_err(|_| ServiceError::invalid_operation("Agent config snapshot is invalid"))?;
        let sandbox_support = snapshot_value
            .get("harness_capabilities")
            .and_then(|value| value.get("capabilities"))
            .and_then(|value| value.get("sandbox_controls"))
            .and_then(Value::as_str);
        if sandbox_support != Some("native") {
            return Err(ServiceError::invalid_operation(
                "Codex read-only sandbox capability is unknown or unsupported",
            ));
        }
        if snapshot_value
            .get(executors::ROUTING_SNAPSHOT_KEY)
            .is_some()
        {
            return Err(ServiceError::invalid_operation(
                "PR6 cannot prove read-only policy across an ordered fallback route",
            ));
        }
        let config = snapshot_value
            .get_mut("config")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| ServiceError::invalid_operation("Codex config snapshot is invalid"))?;
        config.insert("sandbox".to_owned(), Value::String("read-only".to_owned()));
        config.insert(
            "ask_for_approval".to_owned(),
            Value::String("never".to_owned()),
        );
        config.insert(
            "permission_policy".to_owned(),
            Value::String("plan".to_owned()),
        );
        config.insert("auto_commit".to_owned(), Value::Bool(false));
        snapshot_value["permission_policy"] = Value::String("plan".to_owned());
        let started_event_id = new_uuid_v4();
        snapshot_value["pr6_orchestrator_wake"] = serde_json::json!({
            "wake_id": wake.id,
            "event_id": wake.event_id,
            "event_sequence": wake.event_sequence,
            "execution_started_event_id": started_event_id.clone(),
            "work_unit_id": wake.work_unit_id,
            "correlation_id": wake.correlation_id,
            "causation_depth": wake.causation_depth,
            "policy_ref": wake.policy_ref,
            "policy_version": wake.policy_version,
            "policy_digest": wake.policy_digest,
            "task_role_version": wake.task_role_version,
            "task_role_policy_json": wake.task_role_policy_json,
        });
        snapshot = serde_json::to_string(&snapshot_value)
            .map_err(|_| ServiceError::invalid_operation("failed to freeze orchestrator config"))?;

        let parent_execution_id = if let Some(source_event) =
            DomainEventRepo::get_event(&*self.db, &wake.event_id).await?
        {
            if source_event.entity_type == "execution" {
                ExecutionRepo::get_by_id(&*self.db, &source_event.entity_id)
                    .await?
                    .filter(|parent| parent.task_id == task.id)
                    .map(|parent| parent.id)
            } else {
                None
            }
        } else {
            None
        };
        let now = now_rfc3339();
        let create = CreateExecution {
            id: attempt.execution_id.clone(),
            task_id: task.id.clone(),
            agent_id: Some(agent.id.clone()),
            actor_ref: Some(ActorRef::Agent(agent.id.clone())),
            role: "orchestrator".to_owned(),
            purpose: Some(ExecutionPurpose::Orchestrate),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id,
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            summary: Some(prompt),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: Some(snapshot),
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let event = CreateDomainEvent {
            id: started_event_id,
            event_type: "execution.started".to_owned(),
            entity_type: "execution".to_owned(),
            entity_id: create.id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: Some(agent.id.clone()),
            scope_type: "task".to_owned(),
            scope_id: task.id.clone(),
            correlation_id: wake.correlation_id.clone(),
            causation_id: Some(wake.event_id.clone()),
            causation_depth: wake.causation_depth.saturating_add(1).min(16),
            dedupe_key: Some(format!("execution.started:{}", create.id)),
            payload_json: serde_json::json!({
                "execution_id": create.id,
                "task_id": task.id,
                "role": "orchestrator",
                "purpose": "orchestrate",
                "actor_kind": "agent",
                "actor_id": agent.id,
                "work_unit_id": wake.work_unit_id,
                "wake_id": wake.id,
            })
            .to_string(),
            created_at: now,
        };
        let (execution, committed_event) = ExecutionRepo::create_orchestrator_execution(
            &*self.db,
            create,
            &wake.id,
            attempt.attempt_number,
            lease_owner,
            event,
        )
        .await?;
        self.publish_committed_domain_event(&committed_event);
        Ok(execution)
    }

    pub(crate) fn orchestrator_context_path(&self, execution_id: &str) -> PathBuf {
        self.workspace_root
            .join(".forge-orchestrator-context")
            .join(execution_id)
    }
}

impl TaskService {
    pub(crate) fn orchestrator_work_unit_service(&self) -> crate::WorkUnitService {
        let repo_cache_locks = self
            .repo_cache_locks
            .clone()
            .unwrap_or_else(|| Arc::new(RepoCacheLockManager::new()));
        let integration_locks = self
            .workspace_exec_locks
            .clone()
            .unwrap_or_else(|| Arc::new(crate::WorkspaceExecutionLockManager::new()));
        crate::WorkUnitService::new(
            Arc::clone(&self.db),
            Arc::clone(&self.event_bus),
            self.workspace_root.clone(),
            repo_cache_locks,
            integration_locks,
        )
    }
}
