use crate::{models::*, pagination::*, Result};
use async_trait::async_trait;
use sqlx::{Sqlite, Transaction};

#[async_trait]
pub trait TaskRepo: Send + Sync {
    async fn create(&self, input: CreateTask) -> Result<Task>;
    async fn create_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateTask,
    ) -> Result<Task>;
    async fn get_by_id(&self, id: &str, include_deleted: bool) -> Result<Option<Task>>;
    async fn get_by_id_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        include_deleted: bool,
    ) -> Result<Option<Task>>;
    async fn list(&self, query: TaskListQuery) -> Result<Page<Task>>;
    async fn list_by_executing_agent(&self, query: AgentTaskListQuery) -> Result<Page<Task>>;
    async fn list_subtasks_ordered(&self, parent_task_id: &str) -> Result<Vec<Task>>;
    async fn next_subtask_order(&self, parent_task_id: &str) -> Result<i64>;
    async fn next_subtask_order_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        parent_task_id: &str,
    ) -> Result<i64>;
    async fn count_live_children_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        task_id: &str,
    ) -> Result<i64>;
    async fn reorder_subtasks(
        &self,
        parent_task_id: &str,
        ordered_ids: &[String],
        updated_at: &str,
    ) -> Result<()>;
    async fn update(&self, input: UpdateTask) -> Result<Task>;
    async fn update_parent_structure_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: UpdateTask,
        subtask_order: Option<i64>,
    ) -> Result<Task>;
    async fn soft_delete_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: SoftDeleteTask,
    ) -> Result<Task>;
    async fn set_metadata_json(
        &self,
        id: &str,
        metadata_json: Option<String>,
        updated_at: &str,
    ) -> Result<()>;
    async fn set_metadata_json_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        metadata_json: Option<String>,
        updated_at: &str,
    ) -> Result<()>;
}

#[async_trait]
pub trait TaskLifecycleRepo: Send + Sync {
    async fn get_task_lifecycle(&self, task_id: &str) -> Result<Option<TaskLifecycle>>;
    async fn get_task_lifecycle_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        task_id: &str,
    ) -> Result<Option<TaskLifecycle>>;
    async fn list_task_lifecycle_transitions(
        &self,
        task_id: &str,
        limit: i64,
    ) -> Result<Vec<TaskLifecycleTransitionFact>>;
    async fn get_task_lifecycle_transition_fact(
        &self,
        transition_id: &str,
    ) -> Result<Option<TaskLifecycleTransitionFact>>;
    async fn has_task_lifecycle_transition(
        &self,
        task_id: &str,
        idempotency_key: &str,
    ) -> Result<bool>;
    async fn get_task_lifecycle_transition(
        &self,
        identity: TaskLifecycleTransitionIdentity,
    ) -> Result<Option<TaskLifecycleTransitionWrite>>;
    async fn list_lifecycle_migration_audit(
        &self,
        task_id: &str,
    ) -> Result<Option<TaskLifecycleMigrationAudit>>;
    /// The Task projection, lifecycle CAS, causal receipt, and domain event
    /// commit as one transaction. Replays return the original receipt.
    async fn transition_task_lifecycle(
        &self,
        input: TransitionTaskLifecycle,
    ) -> Result<TaskLifecycleTransitionWrite>;
}

#[async_trait]
pub trait GateRepo: Send + Sync {
    async fn create_gate(&self, input: CreateGate) -> Result<CollaborationWrite<Gate>>;
    /// Create a Gate and its first immutable policy revision with both durable
    /// events in one transaction. API creation must not leave an unusable Gate
    /// when policy validation or persistence fails.
    async fn create_gate_with_initial_policy(
        &self,
        gate: CreateGate,
        policy: CreateGatePolicyRevision,
    ) -> Result<(
        CollaborationWrite<Gate>,
        CollaborationWrite<GatePolicyRevision>,
    )>;
    async fn get_gate(&self, id: &str) -> Result<Option<Gate>>;
    async fn list_active_gate_policies(
        &self,
        task_id: &str,
    ) -> Result<Vec<(Gate, GatePolicyRevision)>>;
    /// Find the immutable evaluation emitted while consuming one exact source
    /// event. A replay of that source must leave the evaluation event to apply
    /// its already frozen inputs rather than evaluating newer facts again.
    async fn get_gate_evaluation_for_cause(
        &self,
        gate_id: &str,
        causation_event_id: &str,
    ) -> Result<Option<GateEvaluation>>;
    async fn get_gate_policy_revision(
        &self,
        gate_id: &str,
        revision: i64,
    ) -> Result<Option<GatePolicyRevision>>;
    /// Appends one immutable policy revision and changes the active revision
    /// pointer under compare-and-swap in the same event transaction.
    async fn create_gate_policy_revision(
        &self,
        input: CreateGatePolicyRevision,
    ) -> Result<CollaborationWrite<GatePolicyRevision>>;
    async fn create_gate_evaluation(
        &self,
        input: StoreGateEvaluation,
    ) -> Result<GateEvaluationWrite>;
    async fn get_gate_evaluation(&self, id: &str) -> Result<Option<GateEvaluation>>;
    async fn is_latest_gate_evaluation(
        &self,
        gate_id: &str,
        revision: i64,
        evaluation_id: &str,
    ) -> Result<bool>;
    /// Mutable exact inputs (TaskRole, WorkUnit, and Proposal fences) must
    /// still match before a satisfied evaluation changes lifecycle.
    async fn gate_evaluation_inputs_are_current(&self, evaluation_id: &str) -> Result<bool>;
    async fn list_gate_evaluation_inputs(
        &self,
        evaluation_id: &str,
    ) -> Result<Vec<GateEvaluationInput>>;
}

#[async_trait]
pub trait TaskBoardRepo: Send + Sync {
    async fn board_revision(&self, project_id: &str) -> Result<i64>;
}

#[async_trait]
pub trait AgentRepo: Send + Sync {
    async fn create(&self, input: CreateAgent) -> Result<Agent>;
    async fn create_identity_with_profile(
        &self,
        identity: CreateAgentIdentity,
        profile: CreateAgentProfile,
    ) -> Result<Agent>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Agent>>;
    async fn list(&self, query: AgentListQuery) -> Result<Page<Agent>>;
    async fn list_visible_to_user(
        &self,
        user_id: &str,
        query: AgentListQuery,
    ) -> Result<Page<Agent>>;
    async fn update(&self, input: UpdateAgent) -> Result<Agent>;
    async fn set_paused(&self, id: &str, paused: bool) -> Result<()>;
    async fn duplicate_agent(
        &self,
        source_id: &str,
        new_id: String,
        new_name: String,
        now: String,
    ) -> Result<Agent>;
    async fn archive(&self, id: &str, archived_at: &str) -> Result<()>;
    async fn count_running_executions(&self, agent_id: &str) -> Result<i64>;
}

#[async_trait]
pub trait AgentProfileRepo: Send + Sync {
    async fn create_profile(&self, input: CreateAgentProfile) -> Result<AgentProfile>;
    async fn create_and_select_profile(
        &self,
        profile: CreateAgentProfile,
        selection: SelectAgentProfile,
    ) -> Result<(AgentProfile, Agent)>;
    async fn get_profile(&self, id: &str) -> Result<Option<AgentProfile>>;
    async fn list_profiles(&self, identity_id: &str) -> Result<Vec<AgentProfile>>;
    async fn select_profile(&self, input: SelectAgentProfile) -> Result<Agent>;
}

#[async_trait]
pub trait CredentialHandleRepo: Send + Sync {
    async fn get_credential_handle(&self, id: &str) -> Result<Option<CredentialHandle>>;
    async fn list_credential_handles(&self, owner_user_id: &str) -> Result<Vec<CredentialHandle>>;
    async fn rename_credential_handle(
        &self,
        id: &str,
        owner_user_id: &str,
        label: &str,
        expected_version: i64,
        updated_at: &str,
    ) -> Result<CredentialHandle>;
    /// Agents whose active profile references each of the owner's provider
    /// entries, with the identity's most recent session activity.
    async fn list_credential_usage(&self, owner_user_id: &str) -> Result<Vec<CredentialUsage>>;
    async fn revoke_credential_handle(
        &self,
        id: &str,
        owner_user_id: &str,
        updated_at: &str,
    ) -> Result<CredentialHandle>;
}

#[async_trait]
pub trait ProviderAuthorizationRepo: Send + Sync {
    async fn create_provider_authorization(
        &self,
        input: CreateProviderAuthorizationOperation,
    ) -> Result<ProviderAuthorizationOperation>;
    async fn get_provider_authorization(
        &self,
        id: &str,
        owner_user_id: &str,
    ) -> Result<Option<ProviderAuthorizationOperation>>;
    async fn get_provider_authorization_by_state_hash(
        &self,
        callback_state_hash: &str,
    ) -> Result<Option<ProviderAuthorizationOperation>>;
    async fn update_provider_authorization(
        &self,
        input: UpdateProviderAuthorizationOperation,
    ) -> Result<ProviderAuthorizationOperation>;
}

#[async_trait]
pub trait DomainEventRepo: Send + Sync {
    async fn append_event(&self, input: CreateDomainEvent) -> Result<DomainEvent>;
    async fn append_event_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: &CreateDomainEvent,
    ) -> Result<DomainEvent>;
    async fn get_event(&self, id: &str) -> Result<Option<DomainEvent>>;
    async fn get_event_by_dedupe(&self, dedupe_key: &str) -> Result<Option<DomainEvent>>;
    async fn list_events_after(&self, sequence: i64, limit: i64) -> Result<Vec<DomainEvent>>;
    async fn get_consumer_cursor(&self, consumer_name: &str)
        -> Result<Option<EventConsumerCursor>>;
    async fn claim_event_batch(&self, input: ClaimDomainEvents) -> Result<Vec<DomainEvent>>;
    async fn complete_claimed_event(&self, input: CompleteDomainEvent) -> Result<bool>;
}

#[async_trait]
pub trait OrchestratorWakeRepo: Send + Sync {
    /// Insert one obligation for the exact source event, TaskRole, and Actor.
    /// A uniqueness hit is an idempotent replay and returns `false`.
    async fn admit_orchestrator_wake(&self, input: CreateOrchestratorWake) -> Result<bool>;
    async fn claim_orchestrator_wake(
        &self,
        input: ClaimOrchestratorWake,
    ) -> Result<Option<OrchestratorWake>>;
    async fn get_orchestrator_wake(&self, id: &str) -> Result<Option<OrchestratorWake>>;
    async fn get_orchestrator_wake_by_execution(
        &self,
        execution_id: &str,
    ) -> Result<Option<(OrchestratorWake, OrchestratorWakeExecution)>>;
    async fn reserve_orchestrator_wake_execution(
        &self,
        input: ReserveOrchestratorWakeExecution,
    ) -> Result<OrchestratorWakeExecution>;
    async fn transition_orchestrator_wake(&self, input: TransitionOrchestratorWake)
        -> Result<bool>;
    async fn transition_orchestrator_wake_execution(
        &self,
        input: TransitionOrchestratorWakeExecution,
    ) -> Result<bool>;
    /// Resolve a claimed provider Start and its exact Wake attempt in one
    /// transaction. Only the current lease owner and current attempt may
    /// become running or deterministically failed.
    async fn resolve_orchestrator_start(&self, input: ResolveOrchestratorStart) -> Result<bool>;
    async fn get_orchestrator_action(
        &self,
        execution_id: &str,
        action_index: i64,
    ) -> Result<Option<OrchestratorActionRecord>>;
    async fn reserve_orchestrator_action(
        &self,
        input: ReserveOrchestratorAction,
    ) -> Result<OrchestratorActionRecord>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateDomainEvent {
    pub id: String,
    pub event_type: String,
    pub entity_type: String,
    pub entity_id: String,
    pub actor_type: String,
    pub actor_id: Option<String>,
    pub scope_type: String,
    pub scope_id: String,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub causation_depth: i64,
    pub dedupe_key: Option<String>,
    pub payload_json: String,
    pub created_at: String,
}

impl CreateDomainEvent {
    /// Build the bounded, replayable ledger record for a task status
    /// transition. The transition-log id is deliberately reused as the event
    /// id/correlation key so retries cannot create a second source of truth.
    #[allow(clippy::too_many_arguments)]
    pub fn task_transition(
        transition_log_id: impl Into<String>,
        task_id: impl Into<String>,
        project_id: impl Into<String>,
        from_state: impl AsRef<str>,
        to_state: impl AsRef<str>,
        trigger_name: Option<&str>,
        triggered_by: impl AsRef<str>,
        trigger_reason: impl AsRef<str>,
        rejection: bool,
        created_at: impl Into<String>,
    ) -> Self {
        let transition_log_id = transition_log_id.into();
        let task_id = task_id.into();
        let project_id = project_id.into();
        let from_state = bounded_event_text(from_state.as_ref(), 128);
        let to_state = bounded_event_text(to_state.as_ref(), 128);
        let trigger_name = trigger_name.map(|value| bounded_event_text(value, 128));
        let triggered_by = bounded_event_text(triggered_by.as_ref(), 256);
        let trigger_reason = bounded_event_text(trigger_reason.as_ref(), 512);
        let created_at = created_at.into();
        let (actor_type, actor_id) = triggered_by.split_once(':').map_or_else(
            || (triggered_by.clone(), None),
            |(kind, id)| (kind.to_owned(), Some(id.to_owned())),
        );
        let payload_json = serde_json::json!({
            "transition_log_id": transition_log_id,
            "project_id": project_id,
            "from_state": from_state,
            "to_state": to_state,
            "trigger_name": trigger_name,
            "trigger_reason": trigger_reason,
            "rejection": rejection,
        })
        .to_string();

        Self {
            id: transition_log_id.clone(),
            event_type: "task.transitioned".to_owned(),
            entity_type: "task".to_owned(),
            entity_id: task_id.clone(),
            actor_type,
            actor_id,
            scope_type: "task".to_owned(),
            scope_id: task_id,
            correlation_id: transition_log_id.clone(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("task-transition:{transition_log_id}")),
            payload_json,
            created_at,
        }
    }
}

fn bounded_event_text(value: &str, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value.to_owned();
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = value[..end].to_owned();
    output.push_str("[truncated]");
    output
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimDomainEvents {
    pub consumer_name: String,
    pub lease_owner: String,
    pub now: String,
    pub leased_until: String,
    pub limit: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompleteDomainEvent {
    pub consumer_name: String,
    pub lease_owner: String,
    pub event_sequence: i64,
    pub event_id: String,
    pub dedupe_key: String,
    pub completed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateOrchestratorWake {
    pub id: String,
    pub event_id: String,
    pub event_sequence: i64,
    pub task_id: String,
    pub task_role_id: String,
    pub coordination_mode: Option<CoordinationMode>,
    pub actor_kind: ActorKind,
    pub actor_id: String,
    pub work_unit_id: Option<String>,
    pub correlation_id: String,
    pub causation_id: Option<String>,
    pub causation_depth: i64,
    pub policy_ref: String,
    pub policy_version: i64,
    pub policy_digest: String,
    pub task_role_version: i64,
    pub task_role_policy_json: String,
    pub available_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimOrchestratorWake {
    pub lease_owner: String,
    pub now: String,
    pub leased_until: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveOrchestratorWakeExecution {
    pub wake_id: String,
    pub lease_owner: String,
    pub execution_id: String,
    pub now: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOrchestratorWake {
    pub id: String,
    pub expected_state: Option<OrchestratorWakeState>,
    pub lease_owner: Option<String>,
    pub state: OrchestratorWakeState,
    pub available_at: Option<String>,
    pub current_attempt: Option<Option<i64>>,
    pub last_error: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionOrchestratorWakeExecution {
    pub wake_id: String,
    pub attempt_number: i64,
    pub execution_id: String,
    pub lease_owner: Option<String>,
    pub expected_state: Option<String>,
    pub state: String,
    pub last_error: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveOrchestratorStart {
    pub wake_id: String,
    pub attempt_number: i64,
    pub execution_id: String,
    pub lease_owner: String,
    /// `running` for a durably accepted Start or `failed` for a known
    /// rejection/loss of authority with no external attempt.
    pub state: String,
    pub last_error: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReserveOrchestratorAction {
    pub execution_id: String,
    pub action_index: i64,
    pub action_type: String,
    pub action_digest: String,
    pub result_id: String,
    pub now: String,
}

#[async_trait]
pub trait WorkspaceRepo: Send + Sync {
    async fn create(&self, input: CreateWorkspace) -> Result<Workspace>;
    async fn get_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Workspace>>;
    /// Resolve the Task's explicitly scoped integration Workspace. WorkUnit
    /// workspaces must be addressed by exact WorkUnit or Workspace identity.
    async fn get_integration_by_task_id(&self, task_id: &str) -> Result<Option<Workspace>>;
    async fn set_cleanup_after(
        &self,
        id: &str,
        cleanup_after: Option<String>,
        updated_at: &str,
    ) -> Result<Workspace>;
    /// Atomically claim cleanup for one exact WorkUnit Workspace. Returns
    /// `None` when its lifecycle or active execution/lease authority prevents
    /// cleanup from starting.
    async fn claim_work_unit_cleanup(
        &self,
        id: &str,
        task_id: &str,
        work_unit_id: &str,
        updated_at: &str,
    ) -> Result<Option<Workspace>>;
    /// Complete cleanup only for the exact WorkUnit Workspace currently in
    /// the durable `cleaning` lifecycle state.
    async fn finish_work_unit_cleanup(
        &self,
        id: &str,
        task_id: &str,
        work_unit_id: &str,
        updated_at: &str,
    ) -> Result<Workspace>;
    async fn mark_cleaned(&self, id: &str, updated_at: &str) -> Result<Workspace>;
    async fn list_pending_cleanup(&self, now: &str) -> Result<Vec<Workspace>>;
    async fn update_status(
        &self,
        id: &str,
        status: WorkspaceStatus,
        error: Option<String>,
        updated_at: &str,
    ) -> Result<Workspace>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait WorkUnitWorkspaceRepo: Send + Sync {
    async fn get_scope_by_id(&self, id: &str) -> Result<Option<WorkspaceScope>>;
    async fn get_by_work_unit_id(&self, work_unit_id: &str) -> Result<Option<Workspace>>;
    async fn create_for_work_unit(&self, input: CreateWorkUnitWorkspace) -> Result<Workspace>;
    async fn list_by_task(&self, task_id: &str) -> Result<Vec<Workspace>>;
}

#[async_trait]
pub trait WorkUnitExecutionRepo: Send + Sync {
    async fn create_for_work_unit(
        &self,
        input: CreateWorkUnitExecution,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Execution>> {
        self.create_for_work_unit_with_artifact_inputs(input, Vec::new(), event)
            .await
    }
    async fn create_for_work_unit_with_artifact_inputs(
        &self,
        input: CreateWorkUnitExecution,
        artifact_input_ids: Vec<String>,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Execution>>;
    /// Atomically apply the bounded lifecycle receipt(s), optional recovery
    /// annotation cleanup, and ordinary WorkUnit Execution admission.
    async fn admit_with_lifecycle_and_artifact_inputs(
        &self,
        input: AdmitWorkUnitExecutionWithLifecycle,
        artifact_input_ids: Vec<String>,
        execution_event: CreateDomainEvent,
    ) -> Result<WorkUnitExecutionLifecycleWrite>;
    async fn get_follow_up_execution_receipt(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<FollowUpExecutionReceipt>>;
    /// Atomically clear a blocked Task's recovery projection, transition its
    /// canonical lifecycle to Active, and admit the exact follow-up Execution.
    /// A rejected admission leaves the previous recovery annotation intact.
    async fn admit_recovery_for_work_unit_with_artifact_inputs(
        &self,
        input: RecoverWorkUnitExecution,
        artifact_input_ids: Vec<String>,
        execution_event: CreateDomainEvent,
    ) -> Result<RecoveryWorkUnitExecutionWrite>;
}

#[async_trait]
pub trait WorkUnitRepo: Send + Sync {
    async fn create(
        &self,
        input: CreateWorkUnit,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>>;
    /// Scope-only lookup for the auth-before-load boundary.
    async fn get_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<WorkUnit>>;
    async fn list_by_task(&self, task_id: &str) -> Result<Vec<WorkUnit>>;
    async fn update(
        &self,
        input: UpdateWorkUnit,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>>;
    async fn allocate(
        &self,
        input: AllocateWorkUnit,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>>;
    async fn transition(
        &self,
        input: TransitionWorkUnit,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>>;
    async fn list_dependencies(&self, id: &str) -> Result<Vec<WorkUnitDependency>>;
    async fn list_active_execution_ids(&self, id: &str) -> Result<Vec<String>>;
    async fn add_dependency(
        &self,
        input: AddWorkUnitDependency,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnitDependency>>;
    async fn remove_dependency(
        &self,
        input: RemoveWorkUnitDependency,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>>;
    async fn list_integrations(&self, id: &str) -> Result<Vec<WorkUnitIntegration>>;
    async fn get_integration_by_id(&self, id: &str) -> Result<Option<WorkUnitIntegration>>;
    async fn get_integration_by_idempotency(
        &self,
        task_id: &str,
        key: &str,
    ) -> Result<Option<WorkUnitIntegration>>;
    async fn begin_integration(
        &self,
        input: CreateWorkUnitIntegration,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnitIntegration>>;
    async fn record_integration(
        &self,
        input: RecordWorkUnitIntegration,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnitIntegration>>;
}

/// Durable, Task-wide exclusive authority for operations that read or mutate
/// the Task integration workspace. The partial unique index makes `begin`
/// atomic across independent SQLite connections/processes.
#[async_trait]
pub trait TaskIntegrationOperationRepo: Send + Sync {
    async fn get_by_id(&self, id: &str) -> Result<Option<TaskIntegrationOperation>>;
    async fn begin(
        &self,
        input: CreateTaskIntegrationOperation,
    ) -> Result<TaskIntegrationOperation>;
    /// Atomically admit a PullRequest TaskMerge, create its PublishPr child,
    /// and persist publication intent so a restart can always resume provider
    /// reconciliation from durable rows.
    async fn begin_pull_request_publication(
        &self,
        merge: CreateTaskIntegrationOperation,
        publish: CreateTaskIntegrationOperation,
        metadata: CreatePrMetadata,
        remote_admission: CreateRemotePrAdmission,
    ) -> Result<(TaskIntegrationOperation, TaskIntegrationOperation)>;
    async fn get_remote_pr_admission(
        &self,
        task_merge_operation_id: &str,
    ) -> Result<Option<RemotePrAdmission>>;
    /// Persist a provider observation and its metadata, operation, and
    /// lifecycle effects in one SQLite transaction. Reconciliation-required
    /// outcomes only update the durable admission state.
    async fn record_remote_pr_outcome(
        &self,
        input: RecordRemotePrOutcome,
    ) -> Result<Option<DomainEvent>>;
    async fn get_active_for_task(&self, task_id: &str) -> Result<Option<TaskIntegrationOperation>>;
    /// Called only while holding the Task's OS operation lock. It marks the
    /// current process-dead `running` row abandoned without starting a new
    /// operation, for consumers that only need stale-state reconciliation.
    async fn abandon_stale(
        &self,
        task_id: &str,
        updated_at: &str,
    ) -> Result<Option<TaskIntegrationOperation>>;
    /// Called only while holding the Task's OS operation lock. It atomically
    /// marks a process-dead active row abandoned and installs the replacement.
    async fn recover_stale_and_begin(
        &self,
        input: CreateTaskIntegrationOperation,
        updated_at: &str,
    ) -> Result<TaskIntegrationOperation>;
    async fn finish(
        &self,
        input: FinishTaskIntegrationOperation,
    ) -> Result<TaskIntegrationOperation>;
}

/// Internal scheduler authority for a Task workspace.  A lease is deliberately
/// separate from the filesystem-backed `Workspace` row: chat agents never
/// receive this record, a path, or a bearer token.  The scheduler persists only
/// the opaque logical repository binding and a short-lived capability grant.
#[async_trait]
pub trait WorkspaceLeaseRepo: Send + Sync {
    async fn issue(&self, input: CreateWorkspaceLease) -> Result<WorkspaceLease>;
    async fn get_by_id(&self, id: &str) -> Result<Option<WorkspaceLease>>;
    async fn get_active_for_task(&self, task_id: &str) -> Result<Option<WorkspaceLease>>;
    async fn get_active_for_work_unit(&self, work_unit_id: &str) -> Result<Option<WorkspaceLease>>;
    async fn revoke(
        &self,
        id: &str,
        expected_version: i64,
        revoked_at: &str,
    ) -> Result<WorkspaceLease>;
    /// Extend scheduler-owned leases that are approaching expiry while their
    /// exact Task, execution, assignment, and governance bindings remain
    /// current. Invalid or concurrently changed candidates are skipped.
    async fn renew_active(
        &self,
        now: &str,
        renew_before: &str,
        expires_at: &str,
        limit: i64,
    ) -> Result<Vec<WorkspaceLease>>;
    async fn expire(&self, now: &str, limit: i64) -> Result<Vec<WorkspaceLease>>;
}

#[async_trait]
pub trait DaemonRepo: Send + Sync {
    async fn upsert_by_machine_id(&self, input: UpsertDaemon) -> Result<Daemon>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Daemon>>;
    async fn get_by_machine_id(&self, machine_id: &str) -> Result<Option<Daemon>>;
    async fn list(&self, page: PageRequest) -> Result<Page<Daemon>>;
    async fn list_visible(&self, user_id: Option<&str>, page: PageRequest) -> Result<Page<Daemon>>;
    async fn get_visible(&self, id: &str, user_id: Option<&str>) -> Result<Option<Daemon>>;
    async fn update_report(&self, input: UpdateDaemonReport) -> Result<Daemon>;
    async fn mark_online(&self, id: &str, last_report_at: &str) -> Result<Daemon>;
    async fn mark_offline(&self, id: &str, updated_at: &str) -> Result<Daemon>;
    async fn list_available_for_executor(
        &self,
        executor_type: &str,
        owner_id: &str,
    ) -> Result<Vec<Daemon>>;
}

#[async_trait]
pub trait RuntimeRepo: Send + Sync {
    async fn create(&self, input: CreateRuntime) -> Result<Runtime>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Runtime>>;
    async fn get_by_daemon_id(&self, daemon_id: &str) -> Result<Option<Runtime>>;
    async fn upsert_by_daemon_kind(&self, input: CreateRuntime) -> Result<Runtime>;
    async fn list(&self, query: RuntimeListQuery) -> Result<Page<Runtime>>;
}

#[async_trait]
pub trait ExecutionRepo: Send + Sync {
    async fn create(&self, input: CreateExecution) -> Result<Execution>;
    async fn create_with_event(
        &self,
        input: CreateExecution,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)>;
    /// Create a workspace-bound Human Review Execution, its immutable exact
    /// subject, and the start event in one transaction.
    async fn create_human_review_execution_with_subject(
        &self,
        input: CreateExecution,
        subject: CreateReviewExecutionSubject,
        event: CreateDomainEvent,
    ) -> Result<ReviewExecutionSubjectWrite>;
    /// Create a workspace-free Running Human Review Execution and its start
    /// event in one transaction, rechecking current reviewer authority there.
    async fn create_human_review_execution_without_subject(
        &self,
        input: CreateExecution,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)>;
    /// Freeze the exact subject of a running reviewer Execution, or return
    /// the already-frozen identity only when every field still matches.
    async fn freeze_review_execution_subject(
        &self,
        subject: CreateReviewExecutionSubject,
        updated_at: &str,
        event: CreateDomainEvent,
    ) -> Result<ReviewExecutionSubjectWrite>;
    async fn get_review_execution_subject(
        &self,
        execution_id: &str,
    ) -> Result<Option<ReviewExecutionSubject>>;
    /// Exact bounded lookup used to reuse one running Human Review Execution.
    async fn find_running_human_review_execution(
        &self,
        task_id: &str,
        user_id: &str,
        workspace_id: Option<&str>,
    ) -> Result<Option<Execution>>;
    /// Create a Running Execution, its immutable Artifact inputs, and the
    /// `execution.started` ledger event as one durable boundary.
    async fn create_with_artifact_inputs_and_event(
        &self,
        input: CreateExecution,
        artifact_input_ids: Vec<String>,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)>;
    async fn create_orchestrator_execution(
        &self,
        input: CreateExecution,
        wake_id: &str,
        attempt_number: i64,
        lease_owner: &str,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)>;
    async fn get_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Execution>>;
    /// Return whether migration evidence explicitly marks this historical
    /// Execution's external session identity as contradictory.  This is a
    /// fail-closed compatibility check; it is not a generic session lookup.
    async fn has_historical_session_ambiguity(&self, execution_id: &str) -> Result<bool>;
    async fn stats_by_agent(&self, agent_id: &str) -> Result<AgentExecutionStats>;
    async fn list_by_task(&self, task_id: &str, page: PageRequest) -> Result<Page<Execution>>;
    async fn list_latest_executions_for_tasks(&self, task_ids: &[&str]) -> Result<Vec<Execution>>;
    /// Exact WorkUnit-bound Executions authorized for automatic restart after
    /// an orderly shutdown or process crash. Task-scoped legacy Executions are
    /// intentionally excluded.
    async fn list_work_unit_auto_recovery_candidates(&self) -> Result<Vec<Execution>>;
    /// Newly admitted Agent WorkUnit Executions whose external provider start
    /// has not been claimed. Older admissions without this receipt are not
    /// inferred to be pending because their external outcome is unknowable.
    async fn list_pending_provider_starts(&self) -> Result<Vec<Execution>>;
    /// Check the exact durable queued-start contract for one Execution.
    async fn is_provider_start_pending(&self, execution_id: &str) -> Result<bool>;
    /// Resolve a queued provider Start only while its durable receipt still
    /// proves that no external attempt has been claimed.
    async fn fail_pending_provider_start(
        &self,
        execution_id: &str,
        update: UpdateExecution,
        event: CreateDomainEvent,
    ) -> Result<PendingProviderStartFailure>;
    /// Fail an external execution only if it is still Running and has no
    /// unattempted provider-start receipt at the serialized write boundary.
    async fn fail_running_unless_provider_start_pending(
        &self,
        execution_id: &str,
        update: UpdateExecution,
        event: CreateDomainEvent,
    ) -> Result<RunningExecutionFailure>;
    /// Atomically claim the one external Start attempt for an admitted
    /// Execution. A paused Agent leaves the admission pending; an existing
    /// attempt is returned as accepted, rejected, or uncertain and is never
    /// sent to the provider again.
    async fn claim_provider_start(
        &self,
        execution_id: &str,
        attempt_event: CreateDomainEvent,
    ) -> Result<ProviderStartClaim>;
    /// Persist the provider's observed accepted/rejected result, or record an
    /// uncertain outcome after the request was attempted.
    async fn record_provider_start_outcome(
        &self,
        execution_id: &str,
        outcome_event: CreateDomainEvent,
    ) -> Result<DomainEvent>;
    async fn list_by_task_and_role(
        &self,
        task_id: &str,
        role: &str,
        page: PageRequest,
    ) -> Result<Page<Execution>>;
    async fn count_by_task_and_role(&self, task_id: &str, role: &str) -> Result<i64>;
    async fn update(&self, input: UpdateExecution) -> Result<Execution>;
    async fn update_with_event(
        &self,
        input: UpdateExecution,
        event: CreateDomainEvent,
    ) -> Result<(Execution, DomainEvent)>;
    async fn update_last_activity_at(&self, id: &str, timestamp: &str) -> Result<()>;
    async fn list_stalled_running(&self, stale_before: &str) -> Result<Vec<Execution>>;
    async fn list_running(&self) -> Result<Vec<Execution>>;
    async fn list_running_for_daemon_not_in(
        &self,
        daemon_id: &str,
        created_before: &str,
        exclude_ids: &[String],
    ) -> Result<Vec<Execution>>;
    async fn get_logs_path(&self, id: &str) -> Result<Option<String>>;
    /// Persist an external harness session identity only through the exact
    /// HarnessSession already attached to this Execution.
    async fn record_harness_session_result(
        &self,
        execution_id: &str,
        external_session_id: &str,
        updated_at: &str,
    ) -> Result<Execution>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStartClaim {
    Claimed,
    Accepted,
    Rejected,
    Uncertain,
    AuthorityLost,
    TaskCancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingProviderStartFailure {
    Resolved(Box<Execution>, DomainEvent),
    Attempted(Execution),
    NotPending(Execution),
    NotRunning(Execution),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunningExecutionFailure {
    Failed(Box<Execution>, DomainEvent),
    Pending(Execution),
    NotRunning(Execution),
}

/// Generic PR4 collaboration persistence. Every create/lifecycle operation
/// writes its durable domain event in the same SQLite transaction and returns
/// that committed event for post-commit notification.
#[async_trait]
pub trait CollaborationRepo: Send + Sync {
    async fn create_artifact(
        &self,
        input: CreateArtifact,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Artifact>>;
    /// Persist one immutable output Artifact for an Execution. The database's
    /// `(execution_id, kind)` key makes same-result retries idempotent across
    /// connections and processes.
    async fn create_execution_artifact_output(
        &self,
        input: CreateArtifact,
        event: CreateDomainEvent,
    ) -> Result<ExecutionArtifactOutputWrite>;
    /// Pin a Human Review's complete requested input sets and persist or reuse
    /// its exact ReviewReport and event atomically.
    async fn create_review_report_with_inputs(
        &self,
        input: CreateArtifact,
        evidence_input_ids: Vec<String>,
        artifact_input_ids: Vec<String>,
        event: CreateDomainEvent,
    ) -> Result<ExecutionArtifactOutputWrite>;
    async fn get_execution_artifact_output(
        &self,
        execution_id: &str,
        kind: ArtifactKind,
    ) -> Result<Option<Artifact>>;
    async fn pin_execution_artifact_input(
        &self,
        execution_id: &str,
        artifact_id: &str,
        created_at: &str,
    ) -> Result<ExecutionArtifactInput>;
    async fn list_execution_artifact_inputs(
        &self,
        execution_id: &str,
    ) -> Result<Vec<ExecutionArtifactInput>>;
    async fn get_artifact_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_artifact(&self, id: &str) -> Result<Option<Artifact>>;
    async fn list_artifacts(&self, task_id: &str, page: PageRequest) -> Result<Page<Artifact>>;
    async fn list_artifacts_by_kind(
        &self,
        task_id: &str,
        kind: ArtifactKind,
        page: PageRequest,
    ) -> Result<Page<Artifact>>;

    async fn create_message(
        &self,
        input: CreateMessage,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Message>>;
    async fn get_message_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_message(&self, id: &str) -> Result<Option<Message>>;
    async fn list_messages(&self, task_id: &str, page: PageRequest) -> Result<Page<Message>>;

    async fn create_handoff(
        &self,
        input: CreateHandoff,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Handoff>>;
    async fn get_handoff_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_handoff(&self, id: &str) -> Result<Option<Handoff>>;
    async fn list_handoffs(&self, task_id: &str, page: PageRequest) -> Result<Page<Handoff>>;
    async fn transition_handoff(
        &self,
        input: TransitionHandoff,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Handoff>>;

    async fn create_proposal(
        &self,
        input: CreateProposal,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Proposal>>;
    async fn get_proposal_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_proposal(&self, id: &str) -> Result<Option<Proposal>>;
    async fn list_proposals(&self, task_id: &str, page: PageRequest) -> Result<Page<Proposal>>;
    async fn withdraw_proposal(
        &self,
        id: &str,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Proposal>>;

    async fn create_decision(
        &self,
        input: CreateDecision,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Decision>>;
    async fn get_decision_task_id(&self, id: &str) -> Result<Option<String>>;
    async fn get_decision(&self, id: &str) -> Result<Option<Decision>>;
    async fn list_decisions(&self, task_id: &str, page: PageRequest) -> Result<Page<Decision>>;
}

#[async_trait]
pub trait HarnessSessionRepo: Send + Sync {
    async fn create(&self, input: CreateHarnessSession) -> Result<HarnessSession>;
    async fn get_by_id(&self, id: &str) -> Result<Option<HarnessSession>>;
    async fn find_reusable(
        &self,
        agent_id: &str,
        harness_kind: &str,
        external_session_id: &str,
    ) -> Result<Option<HarnessSession>>;
    async fn update(&self, input: UpdateHarnessSession) -> Result<HarnessSession>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct AgentExecutionStats {
    pub total_runs: i64,
    pub avg_duration_ms: Option<i64>,
    pub success_rate: Option<f64>,
}

#[async_trait]
pub trait ValidationRunRepo: Send + Sync {
    /// Insert the Running identity and `validation_run.started` event atomically.
    /// Same-key retries return the exact existing row and no second event.
    async fn start_validation_run(
        &self,
        input: CreateValidationRun,
        event: CreateDomainEvent,
    ) -> Result<ValidationRunStartWrite>;
    /// Claim or recover an expired Running check lease. The claim token fences
    /// concurrent retries from committing a second result.
    async fn claim_validation_run(
        &self,
        id: &str,
        owner: &str,
        now: &str,
        claim_until: &str,
    ) -> Result<bool>;
    async fn heartbeat_validation_run(
        &self,
        id: &str,
        owner: &str,
        now: &str,
        claim_until: &str,
    ) -> Result<bool>;
    /// Commit terminal status, Evidence, optional validation-report Artifact,
    /// and all domain events as one recoverable database boundary.
    async fn finish_validation_run(
        &self,
        input: FinishValidationRun,
    ) -> Result<ValidationRunCompletionWrite>;
    async fn get_validation_run(&self, id: &str) -> Result<Option<ValidationRun>>;
    async fn get_validation_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<ValidationRun>>;
    async fn list_validation_runs_by_task(&self, task_id: &str) -> Result<Vec<ValidationRun>>;
    /// Select every result for this exact subject; this method has no latest-row semantics.
    async fn list_validation_runs_for_subject(
        &self,
        task_id: &str,
        workspace_id: &str,
        commit_sha: &str,
        workspace_snapshot_digest: &str,
        caused_by_execution_id: Option<&str>,
    ) -> Result<Vec<ValidationRun>>;
    async fn get_evidence(&self, id: &str) -> Result<Option<Evidence>>;
    async fn list_evidence_by_task(&self, task_id: &str) -> Result<Vec<Evidence>>;
    async fn list_evidence_for_validation_run(
        &self,
        validation_run_id: &str,
    ) -> Result<Vec<Evidence>>;
    async fn list_execution_evidence_inputs(
        &self,
        execution_id: &str,
    ) -> Result<Vec<ExecutionEvidenceInput>>;
    async fn get_validation_run_artifact_output(
        &self,
        validation_run_id: &str,
    ) -> Result<Option<Artifact>>;
    async fn pin_execution_evidence_inputs(
        &self,
        execution_id: &str,
        evidence_ids: &[String],
        created_at: &str,
    ) -> Result<Vec<ExecutionEvidenceInput>>;
}

#[async_trait]
pub trait TaskCommentRepo: Send + Sync {
    async fn create_comment(&self, input: CreateTaskComment) -> Result<TaskComment>;
    async fn list_comments(&self, task_id: &str, page: PageRequest) -> Result<Page<TaskComment>>;
    async fn get_comment_by_id(&self, id: &str) -> Result<Option<TaskComment>>;
    async fn delete_comment(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait TaskMediaRepo: Send + Sync {
    async fn create_media(&self, input: CreateTaskMedia) -> Result<TaskMedia>;
    async fn list_media(&self, task_id: &str, page: PageRequest) -> Result<Page<TaskMedia>>;
    async fn list_active_media_for_task(&self, task_id: &str) -> Result<Vec<TaskMedia>>;
    async fn get_media_by_id(&self, id: &str, include_deleted: bool) -> Result<Option<TaskMedia>>;
    async fn soft_delete_media(&self, id: &str, deleted_at: &str) -> Result<TaskMedia>;
}

/// Repository boundary for the additive Project-owned media metadata layer.
///
/// Implementations must keep reference changes transactional. In
/// particular, a cleanup worker may only claim an asset after checking active
/// Task media references and release pins in the same write transaction. Claims carry
/// a persisted owner/expiry lease and asset version; reset/finalize operations
/// must present both exact values.  Every attachment/pin insertion must reject
/// an asset already queued for deletion or marked as deleted.
#[async_trait]
pub trait SharedMediaRepo: Send + Sync {
    async fn get_media_asset(&self, asset_id: &str) -> Result<Option<MediaAsset>>;
    async fn get_media_asset_for_task_media(
        &self,
        task_media_id: &str,
    ) -> Result<Option<MediaAsset>>;
    async fn begin_project_media_upload(
        &self,
        input: BeginProjectMediaUpload,
    ) -> Result<ProjectMediaUpload>;
    async fn list_pending_project_media_uploads(
        &self,
        limit: i64,
    ) -> Result<Vec<ProjectMediaUpload>>;
    async fn delete_pending_project_media_upload(
        &self,
        project_id: &str,
        idempotency_key: &str,
    ) -> Result<bool>;
    async fn create_project_media_asset(
        &self,
        input: CreateProjectMediaAsset,
    ) -> Result<MediaAsset>;
    async fn finalize_project_media_upload(
        &self,
        project_id: &str,
        asset_id: &str,
        now: &str,
    ) -> Result<MediaAsset>;
    /// Persist a checksum discovered from the unchanged on-disk bytes.  The
    /// compare-and-set is intentionally narrow so concurrent reconciliation
    /// cannot overwrite an existing digest.
    async fn set_media_asset_checksum(
        &self,
        asset_id: &str,
        expected_byte_size: i64,
        checksum: &str,
        now: &str,
    ) -> Result<MediaAsset>;
    /// Return purged assets whose bytes still need idempotent physical
    /// cleanup after a crash between the tombstone transaction and unlink.
    async fn list_purged_media_assets(&self, limit: i64) -> Result<Vec<MediaAsset>>;
    async fn mark_purged_media_asset_reconciled(
        &self,
        asset_id: &str,
        reconciled_at: &str,
    ) -> Result<()>;
    /// Resolve an already committed Project media tombstone without applying
    /// a new mutation.  API routes use this before validating the current
    /// authorization so an immutable receipt can be replayed exactly; a
    /// changed receipt returns `IdempotencyConflict`.
    async fn replay_project_media_tombstone(
        &self,
        input: ProjectMediaTombstone,
    ) -> Result<Option<MediaAsset>>;
    async fn tombstone_project_media_asset(
        &self,
        input: ProjectMediaTombstone,
    ) -> Result<MediaAsset>;
    async fn list_project_release_media_pins(
        &self,
        release_id: &str,
    ) -> Result<Vec<ProjectReleaseMediaPin>>;
    async fn reconcile_media_asset(&self, asset_id: &str, now: &str) -> Result<Option<MediaAsset>>;
    async fn claim_media_gc_candidates(
        &self,
        now: &str,
        lease_owner: &str,
        lease_expires_at: &str,
        limit: i64,
    ) -> Result<Vec<MediaAsset>>;
    async fn claim_media_gc_candidate(
        &self,
        asset_id: &str,
        now: &str,
        lease_owner: &str,
        lease_expires_at: &str,
    ) -> Result<Option<MediaAsset>>;
    async fn reset_media_gc_candidate(
        &self,
        asset_id: &str,
        lease_owner: &str,
        expected_version: i64,
        now: &str,
    ) -> Result<Option<MediaAsset>>;
    async fn complete_media_gc(
        &self,
        asset_id: &str,
        lease_owner: &str,
        expected_version: i64,
        deleted_at: &str,
    ) -> Result<Option<MediaAsset>>;
}

#[async_trait]
pub trait TerminalSessionRepo: Send + Sync {
    async fn create_terminal_session(
        &self,
        input: CreateTerminalSession,
    ) -> Result<TerminalSession>;
    async fn get_terminal_session(&self, id: &str) -> Result<Option<TerminalSession>>;
    async fn list_terminal_sessions_for_task(
        &self,
        task_id: &str,
        include_ended: bool,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_running_terminal_sessions_for_task(
        &self,
        task_id: &str,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_running_terminal_sessions_for_user(
        &self,
        user_id: &str,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_running_terminal_sessions_for_workspace(
        &self,
        workspace_id: &str,
    ) -> Result<Vec<TerminalSession>>;
    async fn list_all_running_terminal_sessions(&self) -> Result<Vec<TerminalSession>>;
    async fn update_terminal_session_status(
        &self,
        id: &str,
        expected_version: i64,
        update: UpdateTerminalSessionStatus,
    ) -> Result<TerminalSession>;
    async fn update_terminal_session_size(
        &self,
        id: &str,
        rows: i64,
        cols: i64,
        last_activity_at: &str,
    ) -> Result<TerminalSession>;
    async fn touch_terminal_session_activity(&self, id: &str, last_activity_at: &str)
        -> Result<()>;
    async fn delete_terminal_sessions_for_workspace(&self, workspace_id: &str) -> Result<u64>;
}

#[async_trait]
pub trait ProjectRepo: Send + Sync {
    async fn create(&self, input: CreateProject) -> Result<Project>;
    /// Atomically creates a Project and its canonical owner membership.
    async fn create_with_owner_membership(
        &self,
        input: CreateProject,
        member_id: &str,
    ) -> Result<Project>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Project>>;
    async fn get_by_id_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
    ) -> Result<Option<Project>>;
    async fn set_owner_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        owner_id: &str,
        updated_at: &str,
    ) -> Result<()>;
    async fn list(&self, page: PageRequest) -> Result<Page<Project>>;
    async fn list_visible_to_user(&self, user_id: &str, page: PageRequest)
        -> Result<Page<Project>>;
    async fn update(&self, input: UpdateProject) -> Result<Project>;
    async fn update_at_version(
        &self,
        input: UpdateProject,
        expected_version: i64,
        project_hooks_json: Option<String>,
        caller_user_id: Option<&str>,
    ) -> Result<Project>;
    async fn set_project_hooks_json(
        &self,
        id: &str,
        project_hooks_json: &str,
        updated_at: &str,
    ) -> Result<()>;
    async fn increment_project_work_epoch(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        by: i64,
    ) -> Result<i64>;
    async fn set_paused_at(
        &self,
        id: &str,
        paused_at: Option<String>,
        caller_user_id: Option<&str>,
    ) -> Result<()>;
    /// Deletes a Project. When `caller_user_id` is provided, the exact
    /// owner/admin membership is rechecked inside the deletion transaction.
    async fn delete(&self, id: &str, caller_user_id: Option<&str>) -> Result<()>;
}

#[async_trait]
pub trait ProjectHookRunRepo: Send + Sync {
    async fn try_claim(&self, input: CreateProjectHookRun) -> Result<Option<ProjectHookRun>>;
    async fn try_claim_or_skip_at_limit(
        &self,
        input: CreateProjectHookRun,
        max_active_runs: i64,
        skip_reason: &str,
    ) -> Result<Option<ProjectHookRun>>;
    async fn update_status(&self, input: UpdateProjectHookRun) -> Result<ProjectHookRun>;
    async fn list_recent_for_project(
        &self,
        project_id: &str,
        limit: i64,
    ) -> Result<Vec<ProjectHookRun>>;
    async fn list_for_project(
        &self,
        project_id: &str,
        page: PageRequest,
    ) -> Result<Page<ProjectHookRun>>;
    async fn count_active_for_rule(&self, project_id: &str, rule_id: &str) -> Result<i64>;
}

#[async_trait]
pub trait RepoRepo: Send + Sync {
    async fn create(&self, input: CreateRepo) -> Result<Repo>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Repo>>;
    async fn list_by_project(&self, project_id: &str, page: PageRequest) -> Result<Page<Repo>>;
    async fn update(&self, input: UpdateRepo) -> Result<Repo>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait PrProviderConfigRepo: Send + Sync {
    async fn create(&self, input: CreatePrProviderConfig) -> Result<PrProviderConfig>;
    async fn get_by_repo_id(&self, repo_id: &str) -> Result<Option<PrProviderConfig>>;
    async fn update(&self, input: UpdatePrProviderConfig) -> Result<PrProviderConfig>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait PrMetadataRepo: Send + Sync {
    async fn create(&self, input: CreatePrMetadata) -> Result<PrMetadata>;
    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<PrMetadata>>;
    async fn update(&self, input: UpdatePrMetadata) -> Result<PrMetadata>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait IntegrationRepo: Send + Sync {
    async fn create_integration(
        &self,
        input: CreateProjectIntegration,
    ) -> Result<ProjectIntegration>;
    async fn get_by_id(&self, id: &str) -> Result<Option<ProjectIntegration>>;
    async fn get_by_project_id(&self, project_id: &str) -> Result<Option<ProjectIntegration>>;
    async fn update_integration(
        &self,
        input: UpdateProjectIntegration,
    ) -> Result<ProjectIntegration>;
    async fn update_last_polled_at(
        &self,
        id: &str,
        last_polled_at: &str,
        updated_at: &str,
    ) -> Result<()>;
    async fn list_enabled(&self) -> Result<Vec<ProjectIntegration>>;
    async fn delete_integration(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait ExternalLinkRepo: Send + Sync {
    async fn create_link(&self, input: CreateTaskExternalLink) -> Result<TaskExternalLink>;
    async fn get_by_id(&self, id: &str) -> Result<Option<TaskExternalLink>>;
    async fn get_by_global_id(&self, global_id: &str) -> Result<Option<TaskExternalLink>>;
    async fn get_by_task_id(&self, task_id: &str) -> Result<Option<TaskExternalLink>>;
    async fn list_by_task_id(&self, task_id: &str) -> Result<Vec<TaskExternalLink>>;
    async fn list_by_integration(&self, integration_id: &str) -> Result<Vec<TaskExternalLink>>;
    async fn delete_link(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait SkillRepo: Send + Sync {
    async fn create(&self, input: CreateSkill) -> Result<Skill>;
    async fn get_by_id(&self, id: &str) -> Result<Option<Skill>>;
    async fn list_by_project(&self, project_id: &str, page: PageRequest) -> Result<Page<Skill>>;
    async fn update(&self, input: UpdateSkill) -> Result<Skill>;
    async fn delete(&self, id: &str) -> Result<()>;
}

#[async_trait]
pub trait NotificationRepo: Send + Sync {
    async fn create(&self, input: CreateNotification) -> Result<Notification>;
    async fn list(&self, query: NotificationListQuery) -> Result<Page<Notification>>;
    async fn list_visible_to_user(
        &self,
        user_id: &str,
        query: NotificationListQuery,
    ) -> Result<Page<Notification>>;
    async fn unread_count(&self, project_id: Option<&str>) -> Result<i64>;
    async fn unread_count_visible_to_user(
        &self,
        user_id: &str,
        project_id: Option<&str>,
    ) -> Result<i64>;
    async fn mark_read(&self, id: &str) -> Result<Notification>;
    async fn mark_read_for_user(&self, id: &str, user_id: &str) -> Result<Notification>;
    async fn mark_all_read(&self, project_id: Option<&str>) -> Result<u64>;
    async fn mark_all_read_visible_to_user(
        &self,
        user_id: &str,
        project_id: Option<&str>,
    ) -> Result<u64>;
    async fn delete(&self, id: &str) -> Result<()>;
    async fn delete_for_user(&self, id: &str, user_id: &str) -> Result<()>;
}

#[async_trait]
pub trait TaskDependencyRepo: Send + Sync {
    async fn add_dependency(&self, task_id: &str, depends_on_id: &str, now: &str) -> Result<()>;
    async fn remove_dependency(&self, task_id: &str, depends_on_id: &str) -> Result<()>;
    async fn list_dependencies(&self, task_id: &str) -> Result<Vec<String>>;
    async fn list_dependents(&self, depends_on_id: &str) -> Result<Vec<String>>;
    async fn unsatisfied_dependencies(&self, task_id: &str) -> Result<Vec<String>>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskListQuery {
    pub project_id: String,
    pub q: Option<String>,
    pub lifecycle_states: Vec<TaskLifecycleState>,
    pub agent_ids: Vec<String>,
    pub assignee_types: Vec<String>,
    pub assignee_ids: Vec<String>,
    pub priority: Option<i64>,
    pub include_cancelled: bool,
    pub include_deleted: bool,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTaskListQuery {
    pub agent_id: String,
    pub include_cancelled: bool,
    pub include_deleted: bool,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentListQuery {
    pub status: Option<AgentStatus>,
    pub executor_type: Option<String>,
    pub capabilities: Vec<String>,
    /// Exclude historical native/embedded identities from current Harness
    /// Agent rosters without deleting their durable records.
    pub harness_only: bool,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationListQuery {
    pub project_id: Option<String>,
    pub read: Option<bool>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateProject {
    pub id: String,
    pub name: String,
    pub settings: String,
    pub primary_repo_id: Option<String>,
    pub owner_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateProject {
    pub id: String,
    pub name: Option<String>,
    pub settings: Option<String>,
    pub primary_repo_id: Option<Option<String>>,
    pub paused_at: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRepo {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub remote_url: String,
    pub local_path: Option<String>,
    pub work_mode: WorkMode,
    pub default_branch: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRepo {
    pub id: String,
    pub name: Option<String>,
    pub local_path: Option<Option<String>>,
    pub remote_url: Option<String>,
    pub work_mode: Option<WorkMode>,
    pub default_branch: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePrProviderConfig {
    pub id: String,
    pub repo_id: String,
    pub provider_type: String,
    pub base_url: Option<String>,
    pub polling_interval_seconds: i64,
    pub token_secret_ref: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrProviderConfig {
    pub id: String,
    pub provider_type: Option<String>,
    pub base_url: Option<Option<String>>,
    pub polling_interval_seconds: Option<i64>,
    pub token_secret_ref: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatePrMetadata {
    pub id: String,
    pub task_id: String,
    pub provider_type: String,
    pub provider_pr_id: Option<String>,
    pub pr_url: Option<String>,
    pub source_branch: String,
    pub target_branch: String,
    pub pr_state: String,
    pub merge_status: String,
    pub task_merge_operation_id: String,
    pub publish_operation_id: String,
    pub last_synced_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRemotePrAdmission {
    pub task_merge_operation_id: String,
    pub publish_operation_id: String,
    pub metadata_id: String,
    pub task_id: String,
    pub provider_config_id: String,
    pub provider_type: String,
    pub provider_config_revision: String,
    pub provider_config_digest: String,
    pub provider_base_url: Option<String>,
    pub token_secret_ref: Option<String>,
    pub remote_repo_identity: String,
    pub source_branch: String,
    pub target_branch: String,
    pub admitted_source_sha: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRemotePrOutcome {
    pub expected_task_id: String,
    pub task_merge_operation_id: String,
    pub publish_operation_id: String,
    pub metadata_id: String,
    pub provider_config_id: String,
    pub provider_config_digest: String,
    pub remote_repo_identity: String,
    pub source_branch: String,
    pub target_branch: String,
    /// Provider outcome: open, merged, closed, publication_failed, or
    /// reconciliation_required. A merged result whose observed head differs
    /// from the admitted SHA remains provider status `merged`; the repository
    /// derives `head_mismatch` as a separate integrity classification.
    pub status: String,
    pub provider_event_id: Option<String>,
    pub provider_pr_id: Option<String>,
    pub pr_url: Option<String>,
    pub observed_head_sha: Option<String>,
    pub merged_commit_sha: Option<String>,
    pub reconciliation_reason: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdatePrMetadata {
    pub id: String,
    pub provider_type: Option<String>,
    pub provider_pr_id: Option<Option<String>>,
    pub pr_url: Option<Option<String>>,
    pub source_branch: Option<String>,
    pub target_branch: Option<String>,
    pub pr_state: Option<String>,
    pub merge_status: Option<String>,
    pub task_merge_operation_id: Option<String>,
    pub publish_operation_id: Option<String>,
    pub last_synced_at: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgent {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub executor_type: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities_json: String,
    pub config_json: String,
    pub credential_ref: Option<String>,
    pub daemon_id: Option<String>,
    pub max_concurrent_tasks: i64,
    pub heartbeat_interval_seconds: i64,
    pub max_missed_heartbeats: i64,
    pub status: AgentStatus,
    pub last_heartbeat_at: Option<String>,
    pub is_default: bool,
    pub paused: bool,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgentIdentity {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub max_concurrent_tasks: i64,
    pub heartbeat_interval_seconds: i64,
    pub max_missed_heartbeats: i64,
    pub status: AgentStatus,
    pub last_heartbeat_at: Option<String>,
    pub is_default: bool,
    pub paused: bool,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub account_permission_ceiling: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateAgent {
    pub id: String,
    pub expected_version: i64,
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub model: Option<Option<String>>,
    pub reasoning_effort: Option<Option<String>>,
    pub permission_policy: Option<Option<String>>,
    pub prompt_template: Option<Option<String>>,
    pub capabilities_json: Option<String>,
    pub config_json: Option<String>,
    pub daemon_id: Option<Option<String>>,
    pub max_concurrent_tasks: Option<i64>,
    pub heartbeat_interval_seconds: Option<i64>,
    pub max_missed_heartbeats: Option<i64>,
    pub status: Option<AgentStatus>,
    pub last_heartbeat_at: Option<Option<String>>,
    pub is_default: Option<bool>,
    pub paused: Option<bool>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateAgentProfile {
    pub id: String,
    pub identity_id: String,
    pub backend_kind: String,
    pub executor_type: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub permission_policy: Option<String>,
    pub prompt_template: Option<String>,
    pub capabilities_json: String,
    pub tool_policy_json: String,
    pub config_json: String,
    pub credential_ref: Option<String>,
    pub daemon_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectAgentProfile {
    pub identity_id: String,
    pub profile_id: String,
    pub expected_version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkspace {
    pub id: String,
    pub task_id: String,
    pub repo_id: String,
    pub worktree_path: String,
    pub branch: String,
    pub status: WorkspaceStatus,
    pub before_sha: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkUnitWorkspace {
    pub workspace: CreateWorkspace,
    pub work_unit_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkUnit {
    pub id: String,
    pub task_id: String,
    pub parent_work_unit_id: Option<String>,
    pub title: String,
    pub scope: String,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub requires_integration: bool,
    pub provenance: Option<WorkUnitProvenance>,
    pub created_by: ActorRef,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskIntegrationOperation {
    pub id: String,
    pub task_id: String,
    pub kind: TaskIntegrationOperationKind,
    pub owner_id: String,
    pub gate_evaluation_id: Option<String>,
    pub remote_waiting: bool,
    pub parent_operation_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishTaskIntegrationOperation {
    pub id: String,
    pub expected_version: i64,
    pub status: TaskIntegrationOperationStatus,
    pub result_event_id: Option<String>,
    pub updated_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateWorkUnit {
    pub id: String,
    pub expected_version: i64,
    pub title: Option<String>,
    pub scope: Option<String>,
    pub parent_work_unit_id: Option<Option<String>>,
    pub requires_integration: Option<bool>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllocateWorkUnit {
    pub id: String,
    pub expected_version: i64,
    pub role: String,
    pub assigned_actor: Option<ActorRef>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransitionWorkUnit {
    pub id: String,
    pub expected_version: i64,
    pub status: WorkUnitStatus,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddWorkUnitDependency {
    pub work_unit_id: String,
    pub depends_on_work_unit_id: String,
    pub expected_version: i64,
    pub created_by: ActorRef,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveWorkUnitDependency {
    pub work_unit_id: String,
    pub depends_on_work_unit_id: String,
    pub expected_version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkUnitExecution {
    pub execution: CreateExecution,
    pub work_unit_id: String,
    pub work_unit_version: i64,
    pub workspace_lease: Option<CreateWorkspaceLease>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoverWorkUnitExecution {
    pub admission: CreateWorkUnitExecution,
    pub expected_task_version: i64,
    pub expected_lifecycle_state: TaskLifecycleState,
    pub expected_lifecycle_version: i64,
    pub lifecycle_transition: Option<TransitionTaskLifecycle>,
    /// A human may select one exact cancelled Execution from the current
    /// ambiguous crash-recovery block. The snapshots and selected target are
    /// rechecked inside the same write transaction.
    pub ambiguous_block_selection: Option<ResolveAmbiguousRecoveryBlock>,
    pub expected_error_annotation: Option<String>,
    pub expected_blocked_json: Option<String>,
    pub expected_metadata_json: Option<String>,
    /// Durable DomainEvent receipt for an explicitly requested recovery.
    /// The event and child Execution commit atomically.
    pub request_receipt: Option<CreateDomainEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveAmbiguousRecoveryBlock {
    pub expected_reason_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearTaskBlockingMetadata {
    /// The exact failed Execution whose current TaskLifecycle block this
    /// admission resolves. The WorkUnit and lifecycle receipt are checked in
    /// the same SQLite transaction as the child Execution.
    pub blocking_execution_id: String,
    pub blocking_work_unit_id: String,
    pub expected_lifecycle_reason_kind: String,
    pub expected_error_annotation: Option<String>,
    pub expected_blocked_json: Option<String>,
    pub expected_metadata_json: Option<String>,
    pub replacement_metadata_json: Option<String>,
}

/// A bounded ordinary WorkUnit admission that may activate its Task in the
/// same commit as the Execution. Lifecycle transitions are supplied as
/// receipt-bearing PR9 transitions; this is not a second transition engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmitWorkUnitExecutionWithLifecycle {
    pub admission: CreateWorkUnitExecution,
    pub expected_task_version: i64,
    pub expected_lifecycle_state: TaskLifecycleState,
    pub expected_lifecycle_version: i64,
    pub lifecycle_transitions: Vec<TransitionTaskLifecycle>,
    pub clear_blocking_metadata: Option<ClearTaskBlockingMetadata>,
    pub follow_up_receipt: Option<CreateFollowUpExecutionReceipt>,
    /// Exact parent selected by the automatic recovery dispatcher. The
    /// repository serializes competing admissions and replays its committed
    /// child without creating another Execution.
    pub automatic_recovery_parent_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateFollowUpExecutionReceipt {
    pub idempotency_key: String,
    pub parent_execution_id: String,
    pub request_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FollowUpExecutionReceipt {
    pub idempotency_key: String,
    pub parent_execution_id: String,
    pub request_digest: String,
    pub execution_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkUnitExecutionLifecycleWrite {
    pub execution: Execution,
    pub lifecycle_events: Vec<DomainEvent>,
    pub execution_event: Option<DomainEvent>,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryWorkUnitExecutionWrite {
    pub execution: Execution,
    pub lifecycle_event: Option<DomainEvent>,
    pub execution_event: Option<DomainEvent>,
    pub request_receipt_event: Option<DomainEvent>,
    pub idempotent_replay: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkUnitIntegration {
    pub id: String,
    pub task_id: String,
    pub work_unit_id: String,
    pub execution_id: String,
    pub source_workspace_id: String,
    pub source_branch: String,
    pub source_sha: String,
    pub target_workspace_id: String,
    pub target_branch: String,
    pub target_before_sha: String,
    pub operation_idempotency_key: String,
    pub started_at: String,
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordWorkUnitIntegration {
    pub id: String,
    pub expected_version: i64,
    pub outcome: WorkUnitIntegrationOutcome,
    pub target_after_sha: Option<String>,
    pub conflict_metadata_json: Option<String>,
    pub finished_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateWorkspaceLease {
    pub id: String,
    pub project_id: String,
    pub task_id: String,
    pub work_unit_id: Option<String>,
    pub workspace_id: Option<String>,
    pub task_version: i64,
    pub execution_id: String,
    pub operation_idempotency_key: String,
    pub repository_binding_id: String,
    pub base_ref: String,
    pub role: String,
    pub capabilities_json: String,
    pub assigned_principal_type: String,
    pub assigned_principal_id: String,
    pub capability_profile_revision: String,
    pub capability_profile_digest: String,
    pub issuing_principal_type: String,
    pub issuing_principal_id: String,
    pub issued_at: String,
    pub expires_at: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertDaemon {
    pub id: String,
    pub machine_id: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub agent_version: Option<String>,
    pub labels_json: String,
    pub status: DaemonStatus,
    pub registration_token_hash: Option<String>,
    pub owner_id: Option<String>,
    pub visibility: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateDaemonReport {
    pub id: String,
    pub last_report_at: String,
    pub status: DaemonStatus,
    pub detected_clis_json: String,
    pub labels_json: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateRuntime {
    pub id: String,
    pub daemon_id: String,
    pub kind: String,
    pub workspace_root: String,
    pub status: RuntimeStatus,
    pub labels_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeListQuery {
    pub daemon_id: Option<String>,
    pub page: PageRequest,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateSkill {
    pub id: String,
    pub project_id: String,
    pub name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateSkill {
    pub id: String,
    pub name: Option<String>,
    pub content: Option<String>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTask {
    pub id: String,
    pub project_id: String,
    pub repo_id: Option<String>,
    pub parent_task_id: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub task_type: String,
    pub lifecycle_state: TaskLifecycleState,
    pub is_automation: bool,
    pub priority: i64,
    pub subtask_order: Option<i64>,
    pub merge_config: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateTask {
    pub id: String,
    pub expected_version: i64,
    pub title: Option<String>,
    pub description: Option<Option<String>>,
    pub priority: Option<i64>,
    pub merge_config: Option<Option<String>>,
    pub error_annotation: Option<Option<String>>,
    pub blocked_json: Option<Option<String>>,
    pub failed_json: Option<Option<String>>,
    pub parent_task_id: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftDeleteTask {
    pub id: String,
    pub expected_version: i64,
    pub deleted_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateExecution {
    pub id: String,
    pub task_id: String,
    pub agent_id: Option<String>,
    /// New writers must provide the concrete historical Actor. `None` is
    /// retained only for bounded legacy/test fixture creation and is never a
    /// valid new service-level principal.
    pub actor_ref: Option<ActorRef>,
    pub role: String,
    /// New writers must state the semantic reason for the run. `None` is
    /// retained for historical compatibility rows until PR13 cleanup.
    pub purpose: Option<ExecutionPurpose>,
    pub status: ExecutionStatus,
    pub stop_reason: Option<StopReason>,
    pub stopped_by: Option<String>,
    pub resume_policy: Option<ResumePolicy>,
    pub stopped_at: Option<String>,
    pub parent_execution_id: Option<String>,
    pub harness_session_id: Option<String>,
    pub agent_message_id: Option<String>,
    pub last_activity_at: Option<String>,
    pub summary: Option<String>,
    pub logs_path: Option<String>,
    pub before_sha: Option<String>,
    pub after_sha: Option<String>,
    pub error: Option<String>,
    pub executor_config_snapshot_json: Option<String>,
    pub workspace_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateExecution {
    pub id: String,
    pub status: Option<ExecutionStatus>,
    pub stop_reason: Option<Option<StopReason>>,
    pub stopped_by: Option<Option<String>>,
    pub resume_policy: Option<Option<ResumePolicy>>,
    pub stopped_at: Option<Option<String>>,
    /// External HarnessSession identity observed in an exact Agent Execution
    /// result. It is bound through HarnessSession and is never stored as an
    /// Execution projection.
    pub external_session_id: Option<Option<String>>,
    pub agent_message_id: Option<Option<String>>,
    pub last_activity_at: Option<Option<String>>,
    pub summary: Option<Option<String>>,
    pub logs_path: Option<Option<String>>,
    pub before_sha: Option<Option<String>>,
    pub after_sha: Option<Option<String>>,
    pub error: Option<Option<String>>,
    pub executor_config_snapshot_json: Option<Option<String>>,
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTaskComment {
    pub id: String,
    pub task_id: String,
    pub author_type: CommentAuthorType,
    pub author_id: Option<String>,
    pub author_name: String,
    pub content: String,
    pub created_at: String,
    pub updated_at: String,
}

#[async_trait]
pub trait TaskRoleRepo: Send + Sync {
    async fn create(&self, input: CreateTaskRole) -> std::result::Result<TaskRole, crate::DbError>;
    async fn get_task_id(&self, id: &str) -> std::result::Result<Option<String>, crate::DbError>;
    async fn get_by_task_and_role(
        &self,
        task_id: &str,
        role: &str,
    ) -> std::result::Result<Option<TaskRole>, crate::DbError>;
    async fn get_by_id(&self, id: &str) -> std::result::Result<Option<TaskRole>, crate::DbError>;
    async fn list_by_task(
        &self,
        task_id: &str,
    ) -> std::result::Result<Vec<TaskRole>, crate::DbError>;
    async fn update(&self, input: UpdateTaskRole) -> std::result::Result<TaskRole, crate::DbError>;
}

#[async_trait]
pub trait RoleMembershipRepo: Send + Sync {
    async fn add(
        &self,
        input: CreateRoleMembership,
    ) -> std::result::Result<RoleMembership, crate::DbError>;
    async fn get(&self, id: &str) -> std::result::Result<Option<RoleMembership>, crate::DbError>;
    async fn list_by_role(
        &self,
        task_role_id: &str,
        include_ended: bool,
    ) -> std::result::Result<Vec<RoleMembership>, crate::DbError>;
    async fn list_by_task(
        &self,
        task_id: &str,
        include_ended: bool,
    ) -> std::result::Result<Vec<(TaskRole, RoleMembership)>, crate::DbError>;
    async fn update(
        &self,
        input: UpdateRoleMembership,
    ) -> std::result::Result<RoleMembership, crate::DbError>;
}

#[derive(Debug, Clone)]
pub struct UpsertExecutionUsage {
    pub execution_id: String,
    pub provider: String,
    pub model: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskUsageSummary {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cache_write_tokens: i64,
    pub total_cost_usd: Option<f64>,
    pub execution_count: i64,
}

#[async_trait]
pub trait ExecutionUsageRepo: Send + Sync {
    async fn upsert(&self, input: UpsertExecutionUsage) -> Result<ExecutionUsage>;
    async fn list_by_execution(&self, execution_id: &str) -> Result<Vec<ExecutionUsage>>;
    async fn get_task_usage_summary(&self, task_id: &str) -> Result<TaskUsageSummary>;
}

#[async_trait]
pub trait UserRepo: Send + Sync {
    async fn create_user(&self, user: &User) -> Result<()>;
    async fn get_user_by_id(&self, id: &str) -> Result<Option<User>>;
    async fn get_user_by_email(&self, email: &str) -> Result<Option<User>>;
    async fn search_users(&self, query: &str, limit: i64) -> Result<Vec<User>>;
    async fn list_users(&self, page: PageRequest) -> Result<Page<User>>;
    async fn set_admin(&self, id: &str, is_admin: bool) -> Result<()>;
    /// Demotes an administrator only while another administrator remains.
    async fn set_admin_checked(&self, id: &str, is_admin: bool) -> Result<()>;
    async fn update_profile(
        &self,
        id: &str,
        email: &str,
        display_name: Option<&str>,
        updated_at: &str,
    ) -> Result<()>;
    async fn count_admins(&self) -> Result<i64>;
    async fn delete_user(&self, id: &str) -> Result<bool>;
    /// Deletes a user unless they are the last administrator or the sole owner
    /// of a Project without another owner.
    async fn delete_user_checked(&self, id: &str) -> Result<bool>;
}

#[async_trait]
pub trait RefreshTokenRepo: Send + Sync {
    async fn create_refresh_token(&self, token: &RefreshToken) -> Result<()>;
    async fn delete_refresh_token_by_hash(&self, token_hash: &str) -> Result<Option<RefreshToken>>;
    async fn delete_refresh_tokens_by_family(&self, family_id: &str) -> Result<u64>;
    async fn delete_expired_refresh_tokens(&self) -> Result<u64>;
    async fn get_refresh_tokens_by_user(&self, user_id: &str) -> Result<Vec<RefreshToken>>;
}

#[async_trait]
pub trait PersonalAccessTokenRepo: Send + Sync {
    async fn create_pat(&self, input: CreatePersonalAccessToken) -> Result<PersonalAccessToken>;
    async fn get_pat_by_token_hash(&self, token_hash: &str) -> Result<Option<PersonalAccessToken>>;
    async fn list_pats_by_user(&self, user_id: &str) -> Result<Vec<PersonalAccessToken>>;
    async fn delete_pat(&self, id: &str, user_id: &str) -> Result<()>;
    async fn update_last_used(&self, id: &str, last_used_at: &str) -> Result<()>;
}

#[async_trait]
pub trait OAuthClientRepo: Send + Sync {
    async fn create_client(&self, input: CreateOAuthClient) -> Result<OAuthClient>;
    async fn get_client(&self, client_id: &str) -> Result<Option<OAuthClient>>;
    async fn touch_last_used(&self, client_id: &str, last_used_at: &str) -> Result<()>;
    async fn count_clients_created_since(&self, created_after_rfc3339: &str) -> Result<i64>;
}

#[async_trait]
pub trait OAuthAuthorizationCodeRepo: Send + Sync {
    async fn create_code(
        &self,
        input: CreateOAuthAuthorizationCode,
    ) -> Result<OAuthAuthorizationCode>;
    /// Returns the code row regardless of consumed_at / expires_at; service enforces semantics.
    async fn get_code_by_hash(&self, code_hash: &str) -> Result<Option<OAuthAuthorizationCode>>;
    /// Atomic single-use claim: UPDATE ... WHERE id = ? AND consumed_at IS NULL. Returns true if this caller won.
    async fn mark_code_consumed(&self, id: &str, consumed_at: &str) -> Result<bool>;
    async fn delete_expired_codes(&self, now_rfc3339: &str) -> Result<u64>;
}

#[async_trait]
pub trait OAuthRefreshTokenRepo: Send + Sync {
    async fn create_refresh_token(
        &self,
        input: CreateOAuthRefreshToken,
    ) -> Result<OAuthRefreshToken>;
    async fn create_refresh_token_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateOAuthRefreshToken,
    ) -> Result<OAuthRefreshToken>;
    async fn get_refresh_token_by_hash(
        &self,
        token_hash: &str,
    ) -> Result<Option<OAuthRefreshToken>>;
    /// Atomic single-use claim for rotation. Returns true if this caller revoked the active row.
    async fn claim_refresh_token_for_rotation(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        id: &str,
        revoked_at: &str,
    ) -> Result<bool>;
    /// Set revoked_at on a single row by id. Idempotent.
    async fn revoke_refresh_token(&self, id: &str, revoked_at: &str) -> Result<()>;
    /// Set revoked_at on every non-revoked row in the family. Returns count of newly-revoked rows.
    async fn revoke_refresh_token_family(&self, family_id: &str, revoked_at: &str) -> Result<u64>;
    async fn revoke_refresh_token_family_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        family_id: &str,
        revoked_at: &str,
    ) -> Result<u64>;
    async fn delete_expired_refresh_tokens(&self, now_rfc3339: &str) -> Result<u64>;
}

#[async_trait]
pub trait ProjectMemberRepo: Send + Sync {
    async fn add_member(&self, input: CreateProjectMember) -> Result<ProjectMember>;
    async fn add_member_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        input: CreateProjectMember,
    ) -> Result<ProjectMember>;
    async fn get_member(&self, project_id: &str, user_id: &str) -> Result<Option<ProjectMember>>;
    async fn get_member_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        user_id: &str,
    ) -> Result<Option<ProjectMember>>;
    async fn list_members(&self, project_id: &str) -> Result<Vec<ProjectMember>>;
    async fn list_members_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        project_id: &str,
    ) -> Result<Vec<ProjectMember>>;
    async fn update_member_role(
        &self,
        project_id: &str,
        user_id: &str,
        role: &str,
        updated_at: &str,
    ) -> Result<ProjectMember>;
    async fn update_member_role_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        user_id: &str,
        role: &str,
        updated_at: &str,
    ) -> Result<ProjectMember>;
    async fn remove_member(&self, project_id: &str, user_id: &str) -> Result<()>;
    async fn remove_member_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        user_id: &str,
    ) -> Result<()>;
}

#[async_trait]
pub trait SystemSettingRepo: Send + Sync {
    async fn get_setting(&self, key: &str) -> Result<Option<String>>;
    async fn set_setting(&self, key: &str, value: &str, updated_at: &str) -> Result<()>;
    async fn list_settings(&self) -> Result<Vec<(String, String)>>;
    async fn delete_setting(&self, key: &str) -> Result<()>;
}
