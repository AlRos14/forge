use std::{collections::HashSet, sync::Arc};

use db::{
    new_uuid_v4, now_rfc3339, ActorRef, AgentRepo, Artifact, ArtifactKind, ArtifactStorageKind,
    CollaborationRepo, CollaborationTarget, CreateArtifact, CreateDecision, CreateDomainEvent,
    CreateHandoff, CreateMessage, CreateProposal, Decision, DecisionOutcome, DomainEventRepo,
    ExecutionPurpose, ExecutionRepo, Handoff, HandoffIntent, HandoffStatus, Page, PageRequest,
    Project, ProjectMemberRepo, ProjectRepo, Proposal, ProposalStatus, ProposalTarget,
    ProposalTargetKind, RoleMembershipRepo, RoleMembershipStatus, SqliteDb, Task, TaskRepo,
    TaskRoleRepo, UserRepo, WorkspaceRepo,
};
use events::EventBus;
use sha2::{Digest, Sha256};

use crate::{domain_event_service::DomainEventService, Result, ServiceError};

/// A write principal comes from authenticated Human context or a persisted
/// Execution. API request DTOs never deserialize this value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CollaborationActorSource {
    Human(String),
    Execution(String),
}

#[derive(Debug, Clone)]
pub struct CreateArtifactInput {
    pub task_id: String,
    pub producer_execution_id: String,
    pub kind: ArtifactKind,
    pub storage_kind: ArtifactStorageKind,
    pub content: Option<String>,
    pub content_ref: Option<String>,
    pub metadata_json: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone)]
pub struct CreateMessageInput {
    pub task_id: String,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub body: String,
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CreateHandoffInput {
    pub task_id: String,
    pub source_role_id: Option<String>,
    pub target: CollaborationTarget,
    pub work_unit_id: Option<String>,
    pub intent: HandoffIntent,
    pub parent_execution_id: Option<String>,
    pub expected_policy_ref: Option<String>,
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CreateProposalInput {
    pub task_id: String,
    pub target: ProposalTarget,
    pub action: String,
    pub reason: String,
    pub target_version: Option<i64>,
    pub target_digest: Option<String>,
    pub required_policy_ref: Option<String>,
    pub required_policy_version: Option<i64>,
    pub required_policy_digest: Option<String>,
    pub supersedes_proposal_id: Option<String>,
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CreateDecisionInput {
    pub task_id: String,
    pub proposal_id: String,
    pub proposal_version: i64,
    pub outcome: DecisionOutcome,
    pub rationale: String,
    pub policy_ref: Option<String>,
    pub policy_version: Option<i64>,
    pub policy_digest: Option<String>,
}

struct EventScope<'a> {
    event_type: &'a str,
    entity_type: &'a str,
    entity_id: &'a str,
    task_id: &'a str,
}

#[derive(Clone)]
pub struct CollaborationService {
    db: Arc<SqliteDb>,
    domain_events: DomainEventService,
}

impl CollaborationService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            domain_events: DomainEventService::new(Arc::clone(&db), event_bus),
            db,
        }
    }

    pub async fn create_artifact(
        &self,
        source: CollaborationActorSource,
        input: CreateArtifactInput,
    ) -> Result<Artifact> {
        let (_, actor) = self.authorize_source(&input.task_id, &source).await?;
        validate_artifact_storage(
            input.storage_kind,
            input.content.as_deref(),
            input.content_ref.as_deref(),
        )?;
        let producer_task_id = ExecutionRepo::get_task_id(&*self.db, &input.producer_execution_id)
            .await?
            .ok_or_else(|| not_found("execution", input.producer_execution_id.clone()))?;
        if producer_task_id != input.task_id {
            return Err(not_found("execution", input.producer_execution_id));
        }
        let execution = ExecutionRepo::get_by_id(&*self.db, &input.producer_execution_id)
            .await?
            .ok_or_else(|| not_found("execution", input.producer_execution_id.clone()))?;
        if execution.task_id != input.task_id || execution.actor_ref().as_ref() != Some(&actor) {
            return Err(not_found("execution", input.producer_execution_id));
        }
        if input.kind == ArtifactKind::Plan
            && (execution.purpose != Some(ExecutionPurpose::Plan)
                || input.storage_kind != ArtifactStorageKind::Inline
                || input
                    .content
                    .as_deref()
                    .is_none_or(|content| content.trim().is_empty())
                || input.digest.as_deref().is_none_or(|digest| {
                    digest
                        != hex::encode(Sha256::digest(
                            input.content.as_deref().unwrap_or_default().as_bytes(),
                        ))
                }))
        {
            return Err(invalid(
                "Plan Artifact requires inline content, its exact digest, and a Plan Execution producer",
            ));
        }
        if matches!(&source, CollaborationActorSource::Execution(id) if id != &execution.id) {
            return Err(ServiceError::AuthorizationDenied {
                message: "Artifact producer must be the authenticated Execution".to_owned(),
            });
        }
        let metadata: serde_json::Value = serde_json::from_str(&input.metadata_json)
            .map_err(|_| invalid("Artifact metadata must be a JSON object"))?;
        if !metadata.is_object() {
            return Err(invalid("Artifact metadata must be a JSON object"));
        }
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "artifact.created",
                    entity_type: "artifact",
                    entity_id: &id,
                    task_id: &input.task_id,
                },
                &actor,
                serde_json::json!({
                    "artifact_id": id,
                    "task_id": input.task_id,
                    "kind": input.kind.to_string(),
                }),
                &now,
            )
            .await?;
        let create = CreateArtifact {
            id,
            task_id: input.task_id,
            kind: input.kind,
            storage_kind: input.storage_kind,
            content: input.content,
            content_ref: input.content_ref,
            metadata_json: input.metadata_json,
            digest: input.digest,
            producer_execution_id: execution.id,
            created_at: now,
        };
        if create.kind == ArtifactKind::Plan {
            let write =
                CollaborationRepo::create_execution_artifact_output(&*self.db, create, event)
                    .await?;
            if let Some(event) = write.event.as_ref() {
                self.domain_events.publish_committed(event);
            }
            return Ok(write.artifact);
        }
        let write = CollaborationRepo::create_artifact(&*self.db, create, event).await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    /// Start a Human-authored Plan Execution. The caller must hold the
    /// planner TaskRole; the row has a real Human ActorRef and no
    /// HarnessSession.
    pub async fn start_human_plan_execution(
        &self,
        task_id: &str,
        user_id: &str,
    ) -> Result<db::Execution> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, actor) = self.authorize_source(task_id, &source).await?;
        if !self
            .has_role_name_membership(&task.id, &actor, "planner")
            .await?
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "Human Actor does not hold the planner TaskRole".to_owned(),
            });
        }
        let now = now_rfc3339();
        let input = db::CreateExecution {
            id: new_uuid_v4(),
            task_id: task.id,
            agent_id: None,
            actor_ref: Some(ActorRef::Human(user_id.to_owned())),
            role: "planner".to_owned(),
            purpose: Some(ExecutionPurpose::Plan),
            status: db::ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: None,
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: Some(now.clone()),
            summary: Some("Human plan authoring".to_owned()),
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            created_at: now.clone(),
            updated_at: now,
        };
        let event = crate::task_service::execution_domain_event(&input, "execution.started");
        let (execution, committed_event) =
            ExecutionRepo::create_with_event(&*self.db, input, event).await?;
        self.domain_events.publish_committed(&committed_event);
        Ok(execution)
    }

    /// Complete one Human plan operation. Repeating the same content returns
    /// the same output Artifact and does not attribute the Artifact to a later
    /// approver or create another Execution.
    pub async fn complete_human_plan_execution(
        &self,
        execution_id: &str,
        user_id: &str,
        markdown: &str,
    ) -> Result<Artifact> {
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
        if execution.actor_ref() != Some(ActorRef::Human(user_id.to_owned()))
            || execution.harness_session_id.is_some()
            || execution.agent_id.is_some()
            || execution.purpose != Some(ExecutionPurpose::Plan)
        {
            return Err(ServiceError::AuthorizationDenied {
                message: "Human plan completion must match its Human Plan Execution".to_owned(),
            });
        }
        let source = CollaborationActorSource::Human(user_id.to_owned());
        self.authorize_source(&execution.task_id, &source).await?;
        let artifact = self
            .create_plan_artifact_from_execution(execution_id, markdown)
            .await?;
        if execution.status == db::ExecutionStatus::Running {
            let now = now_rfc3339();
            let event = crate::task_service::execution_status_domain_event(
                &execution,
                &db::ExecutionStatus::Completed,
                &now,
            );
            let (_, committed_event) = ExecutionRepo::update_with_event(
                &*self.db,
                db::UpdateExecution {
                    id: execution.id,
                    status: Some(db::ExecutionStatus::Completed),
                    stop_reason: Some(None),
                    stopped_by: Some(None),
                    resume_policy: Some(None),
                    stopped_at: Some(None),
                    agent_session_id: Some(None),
                    agent_message_id: Some(None),
                    last_activity_at: Some(Some(now.clone())),
                    summary: Some(Some("Human plan saved".to_owned())),
                    logs_path: Some(None),
                    before_sha: Some(None),
                    after_sha: Some(None),
                    error: Some(None),
                    executor_config_snapshot_json: Some(None),
                    updated_at: now,
                },
                event,
            )
            .await?;
            self.domain_events.publish_committed(&committed_event);
        } else if execution.status != db::ExecutionStatus::Completed {
            return Err(ServiceError::invalid_operation(
                "Human Plan Execution must be running or completed",
            ));
        }
        Ok(artifact)
    }

    /// Materialize the full result of one exact Plan Execution. The generic
    /// output binding serializes competing retries and returns the existing
    /// Artifact only when the complete result is identical.
    pub async fn create_plan_artifact_from_execution(
        &self,
        execution_id: &str,
        markdown: &str,
    ) -> Result<Artifact> {
        if markdown.trim().is_empty() {
            return Err(invalid("Plan Execution result must contain content"));
        }
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
        if execution.purpose != Some(ExecutionPurpose::Plan)
            || !matches!(
                execution.status,
                db::ExecutionStatus::Running | db::ExecutionStatus::Completed
            )
        {
            return Err(invalid(
                "Plan Artifact producer must be a running or completed Plan Execution",
            ));
        }
        let actor = execution
            .actor_ref()
            .ok_or_else(|| invalid("Plan Execution has no persisted ActorRef"))?;
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| not_found("task", execution.task_id.clone()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| not_found("project", task.project_id.clone()))?;
        match &actor {
            ActorRef::Human(user_id) => self.authorize_human_project(&project, user_id).await?,
            ActorRef::Agent(agent_id) => {
                if AgentRepo::get_by_id(&*self.db, agent_id).await?.is_none() {
                    return Err(invalid("Plan Execution ActorRef no longer exists"));
                }
            }
        }

        let source = CollaborationActorSource::Execution(execution.id.clone());
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let digest = hex::encode(Sha256::digest(markdown.as_bytes()));
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "artifact.created",
                    entity_type: "artifact",
                    entity_id: &id,
                    task_id: &execution.task_id,
                },
                &actor,
                serde_json::json!({
                    "artifact_id": id,
                    "task_id": execution.task_id,
                    "kind": "plan",
                }),
                &now,
            )
            .await?;
        let write = CollaborationRepo::create_execution_artifact_output(
            &*self.db,
            CreateArtifact {
                id,
                task_id: execution.task_id,
                kind: ArtifactKind::Plan,
                storage_kind: ArtifactStorageKind::Inline,
                content: Some(markdown.to_owned()),
                content_ref: None,
                metadata_json: "{}".to_owned(),
                digest: Some(digest),
                producer_execution_id: execution.id,
                created_at: now,
            },
            event,
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok(write.artifact)
    }

    pub async fn pin_execution_artifact_input(
        &self,
        execution_id: &str,
        artifact_id: &str,
    ) -> Result<db::ExecutionArtifactInput> {
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
        if execution.status != db::ExecutionStatus::Running || execution.logs_path.is_some() {
            return Err(ServiceError::invalid_operation(
                "Execution Artifact inputs must be pinned before dispatch starts",
            ));
        }
        let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
            .await?
            .ok_or_else(|| not_found("artifact", artifact_id.to_owned()))?;
        if artifact.task_id != execution.task_id {
            return Err(not_found("artifact", artifact_id.to_owned()));
        }
        let binding = CollaborationRepo::pin_execution_artifact_input(
            &*self.db,
            execution_id,
            artifact_id,
            &now_rfc3339(),
        )
        .await?;
        Ok(binding)
    }

    pub async fn get_artifact(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<Artifact> {
        let task_id = CollaborationRepo::get_artifact_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("artifact", id.to_owned()))?;
        self.authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("artifact", id.to_owned()))?;
        CollaborationRepo::get_artifact(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("artifact", id.to_owned()))
    }

    pub async fn list_artifacts(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        page: PageRequest,
    ) -> Result<Page<Artifact>> {
        self.authorize_source(task_id, &source).await?;
        Ok(CollaborationRepo::list_artifacts(&*self.db, task_id, page).await?)
    }

    pub async fn create_message(
        &self,
        source: CollaborationActorSource,
        input: CreateMessageInput,
    ) -> Result<db::Message> {
        self.create_message_with_id(source, input, new_uuid_v4())
            .await
    }

    pub(crate) async fn create_message_with_id(
        &self,
        source: CollaborationActorSource,
        input: CreateMessageInput,
        id: String,
    ) -> Result<db::Message> {
        let (task, sender) = self.authorize_source(&input.task_id, &source).await?;
        if input.body.trim().is_empty() {
            return Err(invalid("Message body must not be empty"));
        }
        self.validate_target(&task, &input.target).await?;
        self.validate_work_unit_context(&task, input.work_unit_id.as_deref())
            .await?;
        self.validate_artifact_links(&task.id, &input.artifact_ids)
            .await?;
        if let Some(existing) = CollaborationRepo::get_message(&*self.db, &id).await? {
            if message_matches(&existing, &task.id, &sender, &input) {
                return Ok(existing);
            }
            return Err(invalid(
                "Message idempotency key conflicts with an existing record",
            ));
        }
        let now = now_rfc3339();
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "message.created",
                    entity_type: "message",
                    entity_id: &id,
                    task_id: &task.id,
                },
                &sender,
                serde_json::json!({
                    "message_id": id,
                    "task_id": task.id,
                    "target_kind": input.target.kind().to_string(),
                    "work_unit_id": input.work_unit_id.clone(),
                }),
                &now,
            )
            .await?;
        let expected = CreateMessage {
            id,
            task_id: task.id,
            sender,
            target: input.target,
            work_unit_id: input.work_unit_id,
            body: input.body,
            artifact_ids: input.artifact_ids,
            created_at: now,
        };
        let write =
            match CollaborationRepo::create_message(&*self.db, expected.clone(), event).await {
                Ok(write) => write,
                Err(error) => {
                    if let Some(existing) =
                        CollaborationRepo::get_message(&*self.db, &expected.id).await?
                    {
                        if message_matches_record(&existing, &expected) {
                            return Ok(existing);
                        }
                    }
                    return Err(error.into());
                }
            };
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_message(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<db::Message> {
        let task_id = CollaborationRepo::get_message_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("message", id.to_owned()))?;
        self.authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("message", id.to_owned()))?;
        CollaborationRepo::get_message(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("message", id.to_owned()))
    }

    pub async fn list_messages(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        page: PageRequest,
    ) -> Result<Page<db::Message>> {
        self.authorize_source(task_id, &source).await?;
        Ok(CollaborationRepo::list_messages(&*self.db, task_id, page).await?)
    }

    pub async fn create_handoff(
        &self,
        source: CollaborationActorSource,
        input: CreateHandoffInput,
    ) -> Result<Handoff> {
        self.create_handoff_with_id(source, input, new_uuid_v4())
            .await
    }

    pub(crate) async fn create_handoff_with_id(
        &self,
        source: CollaborationActorSource,
        input: CreateHandoffInput,
        id: String,
    ) -> Result<Handoff> {
        let (task, created_by) = self.authorize_source(&input.task_id, &source).await?;
        self.validate_target(&task, &input.target).await?;
        self.validate_work_unit_context(&task, input.work_unit_id.as_deref())
            .await?;
        self.validate_artifact_links(&task.id, &input.artifact_ids)
            .await?;
        let (source_role_id, parent_execution_id) = match &source {
            CollaborationActorSource::Human(user_id) => {
                if let Some(role_id) = input.source_role_id.as_deref() {
                    self.require_membership(
                        &task.id,
                        &ActorRef::Human(user_id.clone()),
                        Some(role_id),
                    )
                    .await?;
                }
                if let Some(execution_id) = input.parent_execution_id.as_deref() {
                    let parent_task_id = ExecutionRepo::get_task_id(&*self.db, execution_id)
                        .await?
                        .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
                    if parent_task_id != task.id {
                        return Err(not_found("execution", execution_id.to_owned()));
                    }
                    let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                        .await?
                        .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
                    if execution.task_id != task.id
                        || execution.actor_ref().as_ref() != Some(&created_by)
                    {
                        return Err(not_found("execution", execution_id.to_owned()));
                    }
                }
                (input.source_role_id, input.parent_execution_id)
            }
            CollaborationActorSource::Execution(execution_id) => {
                if input
                    .parent_execution_id
                    .as_deref()
                    .is_some_and(|id| id != execution_id)
                {
                    return Err(invalid(
                        "Agent Handoff parent must be its current Execution",
                    ));
                }
                let source_task_id = ExecutionRepo::get_task_id(&*self.db, execution_id)
                    .await?
                    .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
                if source_task_id != task.id {
                    return Err(not_found("execution", execution_id.to_owned()));
                }
                let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                    .await?
                    .ok_or_else(|| not_found("execution", execution_id.to_owned()))?;
                let role = db::canonical_task_role_name(&execution.role)
                    .ok_or_else(|| invalid("Execution role cannot author a Handoff"))?;
                let task_role = TaskRoleRepo::get_by_task_and_role(&*self.db, &task.id, &role)
                    .await?
                    .ok_or_else(|| invalid("Execution TaskRole is not persisted"))?;
                if input
                    .source_role_id
                    .as_deref()
                    .is_some_and(|id| id != task_role.id)
                {
                    return Err(invalid(
                        "Agent Handoff source role is derived from Execution",
                    ));
                }
                (Some(task_role.id), Some(execution.id))
            }
        };
        let now = now_rfc3339();
        let expected = CreateHandoff {
            id,
            task_id: task.id,
            created_by,
            source_role_id,
            target: input.target,
            work_unit_id: input.work_unit_id,
            intent: input.intent,
            parent_execution_id,
            expected_policy_ref: input.expected_policy_ref,
            artifact_ids: input.artifact_ids,
            created_at: now.clone(),
        };
        if let Some(existing) = CollaborationRepo::get_handoff(&*self.db, &expected.id).await? {
            if handoff_matches_record(&existing, &expected) {
                return Ok(existing);
            }
            return Err(invalid(
                "Handoff idempotency key conflicts with an existing record",
            ));
        }
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "handoff.created",
                    entity_type: "handoff",
                    entity_id: &expected.id,
                    task_id: &expected.task_id,
                },
                &expected.created_by,
                serde_json::json!({
                    "handoff_id": expected.id,
                    "task_id": expected.task_id,
                    "intent": expected.intent.to_string(),
                    "target_kind": expected.target.kind().to_string(),
                    "work_unit_id": expected.work_unit_id,
                }),
                &now,
            )
            .await?;
        let write =
            match CollaborationRepo::create_handoff(&*self.db, expected.clone(), event).await {
                Ok(write) => write,
                Err(error) => {
                    if let Some(existing) =
                        CollaborationRepo::get_handoff(&*self.db, &expected.id).await?
                    {
                        if handoff_matches_record(&existing, &expected) {
                            return Ok(existing);
                        }
                    }
                    return Err(error.into());
                }
            };
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_handoff(&self, id: &str, source: CollaborationActorSource) -> Result<Handoff> {
        let task_id = CollaborationRepo::get_handoff_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("handoff", id.to_owned()))?;
        self.authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("handoff", id.to_owned()))?;
        CollaborationRepo::get_handoff(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("handoff", id.to_owned()))
    }

    pub async fn list_handoffs(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        page: PageRequest,
    ) -> Result<Page<Handoff>> {
        self.authorize_source(task_id, &source).await?;
        Ok(CollaborationRepo::list_handoffs(&*self.db, task_id, page).await?)
    }

    pub async fn transition_handoff(
        &self,
        source: CollaborationActorSource,
        id: &str,
        expected_version: i64,
        next_status: HandoffStatus,
    ) -> Result<Handoff> {
        let task_id = CollaborationRepo::get_handoff_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("handoff", id.to_owned()))?;
        let (_, actor) = self
            .authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("handoff", id.to_owned()))?;
        let existing = CollaborationRepo::get_handoff(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("handoff", id.to_owned()))?;
        let transition_allowed = matches!(
            (existing.status, next_status),
            (HandoffStatus::Pending, HandoffStatus::Accepted)
                | (HandoffStatus::Pending, HandoffStatus::Declined)
                | (HandoffStatus::Pending, HandoffStatus::Cancelled)
                | (HandoffStatus::Accepted, HandoffStatus::Completed)
                | (HandoffStatus::Accepted, HandoffStatus::Cancelled)
        );
        if !transition_allowed {
            return Err(invalid("Handoff lifecycle transition is invalid"));
        }
        self.authorize_handoff_transition(&existing, &actor, next_status)
            .await?;
        let now = now_rfc3339();
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "handoff.status_changed",
                    entity_type: "handoff",
                    entity_id: id,
                    task_id: &existing.task_id,
                },
                &actor,
                serde_json::json!({
                    "handoff_id": id,
                    "task_id": existing.task_id,
                    "from_status": existing.status.to_string(),
                    "status": next_status.to_string(),
                    "version": existing.version + 1,
                }),
                &now,
            )
            .await?;
        let write = CollaborationRepo::transition_handoff(
            &*self.db,
            db::TransitionHandoff {
                id: id.to_owned(),
                expected_version,
                status: next_status,
                updated_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn create_proposal(
        &self,
        source: CollaborationActorSource,
        input: CreateProposalInput,
    ) -> Result<Proposal> {
        self.create_proposal_with_id(source, input, new_uuid_v4())
            .await
    }

    pub(crate) async fn create_proposal_with_id(
        &self,
        source: CollaborationActorSource,
        input: CreateProposalInput,
        id: String,
    ) -> Result<Proposal> {
        let (task, proposer) = self.authorize_source(&input.task_id, &source).await?;
        if input.action.trim().is_empty() {
            return Err(invalid("Proposal action must not be empty"));
        }
        self.validate_proposal_target(&task, &input.target).await?;
        if let Some(prior_id) = input.supersedes_proposal_id.as_deref() {
            let prior_task_id = CollaborationRepo::get_proposal_task_id(&*self.db, prior_id)
                .await?
                .ok_or_else(|| not_found("proposal", prior_id.to_owned()))?;
            if prior_task_id != task.id {
                return Err(not_found("proposal", prior_id.to_owned()));
            }
            let prior = CollaborationRepo::get_proposal(&*self.db, prior_id)
                .await?
                .ok_or_else(|| not_found("proposal", prior_id.to_owned()))?;
            if prior.status != ProposalStatus::Superseded {
                return Err(invalid(
                    "superseded Proposal must have a supersede Decision",
                ));
            }
        }
        self.validate_artifact_links(&task.id, &input.artifact_ids)
            .await?;
        let now = now_rfc3339();
        let expected = CreateProposal {
            id,
            task_id: task.id,
            proposer,
            target: input.target,
            action: input.action,
            reason: input.reason,
            target_version: input.target_version,
            target_digest: input.target_digest,
            required_policy_ref: input.required_policy_ref,
            required_policy_version: input.required_policy_version,
            required_policy_digest: input.required_policy_digest,
            supersedes_proposal_id: input.supersedes_proposal_id,
            artifact_ids: input.artifact_ids,
            created_at: now.clone(),
        };
        if let Some(existing) = CollaborationRepo::get_proposal(&*self.db, &expected.id).await? {
            if proposal_matches_record(&existing, &expected) {
                return Ok(existing);
            }
            return Err(invalid(
                "Proposal idempotency key conflicts with an existing record",
            ));
        }
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "proposal.created",
                    entity_type: "proposal",
                    entity_id: &expected.id,
                    task_id: &expected.task_id,
                },
                &expected.proposer,
                serde_json::json!({
                    "proposal_id": expected.id,
                    "task_id": expected.task_id,
                    "target_kind": expected.target.kind.to_string(),
                }),
                &now,
            )
            .await?;
        let write =
            match CollaborationRepo::create_proposal(&*self.db, expected.clone(), event).await {
                Ok(write) => write,
                Err(error) => {
                    if let Some(existing) =
                        CollaborationRepo::get_proposal(&*self.db, &expected.id).await?
                    {
                        if proposal_matches_record(&existing, &expected) {
                            return Ok(existing);
                        }
                    }
                    return Err(error.into());
                }
            };
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_proposal(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<Proposal> {
        let task_id = CollaborationRepo::get_proposal_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("proposal", id.to_owned()))?;
        self.authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("proposal", id.to_owned()))?;
        CollaborationRepo::get_proposal(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("proposal", id.to_owned()))
    }

    pub async fn list_proposals(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        page: PageRequest,
    ) -> Result<Page<Proposal>> {
        self.authorize_source(task_id, &source).await?;
        Ok(CollaborationRepo::list_proposals(&*self.db, task_id, page).await?)
    }

    pub async fn withdraw_proposal(
        &self,
        source: CollaborationActorSource,
        id: &str,
    ) -> Result<Proposal> {
        let task_id = CollaborationRepo::get_proposal_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("proposal", id.to_owned()))?;
        let (_, actor) = self
            .authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("proposal", id.to_owned()))?;
        let existing = CollaborationRepo::get_proposal(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("proposal", id.to_owned()))?;
        if existing.proposer != actor {
            return Err(not_found("proposal", id.to_owned()));
        }
        let now = now_rfc3339();
        let event = self
            .event_from_source(
                &source,
                EventScope {
                    event_type: "proposal.withdrawn",
                    entity_type: "proposal",
                    entity_id: id,
                    task_id: &existing.task_id,
                },
                &actor,
                serde_json::json!({
                    "proposal_id": id,
                    "task_id": existing.task_id,
                    "status": "withdrawn",
                }),
                &now,
            )
            .await?;
        let write = CollaborationRepo::withdraw_proposal(&*self.db, id, event).await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    /// These identity sources are trusted server-side contexts. A request DTO
    /// cannot provide decider IDs; Agents resolve through same-Task Execution.
    pub async fn record_decision(
        &self,
        input: CreateDecisionInput,
        deciders: Vec<CollaborationActorSource>,
    ) -> Result<Decision> {
        if deciders.is_empty() {
            return Err(invalid("Decision requires at least one decider"));
        }
        let mut actors = Vec::with_capacity(deciders.len());
        let mut identities = HashSet::new();
        for source in &deciders {
            let (_, actor) = self.authorize_source(&input.task_id, source).await?;
            if identities.insert((actor.kind().to_string(), actor.id().to_owned())) {
                actors.push(actor);
            }
        }
        if actors.is_empty() {
            return Err(invalid("Decision requires at least one distinct decider"));
        }
        let proposal_task_id =
            CollaborationRepo::get_proposal_task_id(&*self.db, &input.proposal_id)
                .await?
                .ok_or_else(|| not_found("proposal", input.proposal_id.clone()))?;
        if proposal_task_id != input.task_id {
            return Err(not_found("proposal", input.proposal_id));
        }
        let proposal = CollaborationRepo::get_proposal(&*self.db, &input.proposal_id)
            .await?
            .ok_or_else(|| not_found("proposal", input.proposal_id.clone()))?;
        if proposal.content_version != input.proposal_version
            || proposal.status != ProposalStatus::Open
        {
            return Err(invalid("Decision Proposal is stale or already resolved"));
        }
        let initiating_actor = actors[0].clone();
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let source = deciders
            .first()
            .ok_or_else(|| invalid("Decision requires a decider"))?;
        let event = self
            .event_from_source(
                source,
                EventScope {
                    event_type: "decision.recorded",
                    entity_type: "decision",
                    entity_id: &id,
                    task_id: &input.task_id,
                },
                &initiating_actor,
                serde_json::json!({
                    "decision_id": id,
                    "task_id": input.task_id,
                    "proposal_id": input.proposal_id,
                    "outcome": input.outcome.to_string(),
                }),
                &now,
            )
            .await?;
        let write = CollaborationRepo::create_decision(
            &*self.db,
            CreateDecision {
                id,
                task_id: input.task_id,
                proposal_id: input.proposal_id,
                proposal_version: input.proposal_version,
                outcome: input.outcome,
                rationale: input.rationale,
                policy_ref: input.policy_ref,
                policy_version: input.policy_version,
                policy_digest: input.policy_digest,
                actors,
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_decision(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<Decision> {
        let task_id = CollaborationRepo::get_decision_task_id(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("decision", id.to_owned()))?;
        self.authorize_source(&task_id, &source)
            .await
            .map_err(|_| not_found("decision", id.to_owned()))?;
        CollaborationRepo::get_decision(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("decision", id.to_owned()))
    }

    pub async fn list_decisions(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        page: PageRequest,
    ) -> Result<Page<Decision>> {
        self.authorize_source(task_id, &source).await?;
        Ok(CollaborationRepo::list_decisions(&*self.db, task_id, page).await?)
    }

    pub(crate) async fn authorize_source(
        &self,
        task_id: &str,
        source: &CollaborationActorSource,
    ) -> Result<(Task, ActorRef)> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| not_found("task", task_id.to_owned()))?;
        let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| not_found("task", task_id.to_owned()))?;
        let actor = match source {
            CollaborationActorSource::Human(user_id) => {
                self.authorize_human_project(&project, user_id)
                    .await
                    .map_err(|_| not_found("task", task_id.to_owned()))?;
                ActorRef::Human(user_id.clone())
            }
            CollaborationActorSource::Execution(execution_id) => {
                let execution_task_id = ExecutionRepo::get_task_id(&*self.db, execution_id)
                    .await?
                    .ok_or_else(|| not_found("execution", execution_id.clone()))?;
                if execution_task_id != task.id {
                    return Err(not_found("execution", execution_id.clone()));
                }
                let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                    .await?
                    .ok_or_else(|| not_found("execution", execution_id.clone()))?;
                if execution.task_id != task.id {
                    return Err(not_found("execution", execution_id.clone()));
                }
                let actor = execution
                    .actor_ref()
                    .ok_or_else(|| invalid("Execution has no persisted ActorRef"))?;
                match &actor {
                    ActorRef::Human(user_id) => {
                        self.authorize_human_project(&project, user_id)
                            .await
                            .map_err(|_| not_found("execution", execution_id.clone()))?;
                    }
                    ActorRef::Agent(agent_id) => {
                        let role = db::canonical_task_role_name(&execution.role)
                            .ok_or_else(|| invalid("Execution has no addressable TaskRole"))?;
                        if !self
                            .has_role_name_membership(&task.id, &actor, &role)
                            .await?
                        {
                            return Err(ServiceError::AuthorizationDenied {
                                message: "Execution Actor has no active TaskRole membership"
                                    .to_owned(),
                            });
                        }
                        if AgentRepo::get_by_id(&*self.db, agent_id).await?.is_none() {
                            return Err(invalid("Execution ActorRef no longer exists"));
                        }
                    }
                }
                actor
            }
        };
        Ok((task, actor))
    }

    async fn authorize_human_project(&self, project: &Project, user_id: &str) -> Result<()> {
        let member = ProjectMemberRepo::get_member(&*self.db, &project.id, user_id)
            .await?
            .is_some();
        // This matches the current local-first Project access contract.
        if project.owner_id.is_some() && project.owner_id.as_deref() != Some(user_id) && !member {
            return Err(not_found("project", project.id.clone()));
        }
        if UserRepo::get_user_by_id(&*self.db, user_id)
            .await?
            .is_none()
        {
            return Err(not_found("project", project.id.clone()));
        }
        Ok(())
    }

    async fn validate_target(&self, task: &Task, target: &CollaborationTarget) -> Result<()> {
        match target {
            CollaborationTarget::Task => Ok(()),
            CollaborationTarget::Role(role_id) => {
                let role_task_id = TaskRoleRepo::get_task_id(&*self.db, role_id)
                    .await?
                    .ok_or_else(|| not_found("task_role", role_id.clone()))?;
                if role_task_id != task.id {
                    return Err(not_found("task_role", role_id.clone()));
                }
                let role = TaskRoleRepo::get_by_id(&*self.db, role_id)
                    .await?
                    .ok_or_else(|| not_found("task_role", role_id.clone()))?;
                if role.task_id != task.id {
                    return Err(not_found("task_role", role_id.clone()));
                }
                Ok(())
            }
            CollaborationTarget::Actor(actor) => match actor {
                ActorRef::Human(user_id) => {
                    let project = ProjectRepo::get_by_id(&*self.db, &task.project_id)
                        .await?
                        .ok_or_else(|| not_found("task", task.id.clone()))?;
                    self.authorize_human_project(&project, user_id)
                        .await
                        .map_err(|_| not_found("actor", user_id.clone()))
                }
                ActorRef::Agent(agent_id) => {
                    if !self.has_membership(&task.id, actor, None).await? {
                        return Err(not_found("actor", agent_id.clone()));
                    }
                    if AgentRepo::get_by_id(&*self.db, agent_id).await?.is_none() {
                        return Err(not_found("actor", agent_id.clone()));
                    }
                    Ok(())
                }
            },
        }
    }

    async fn validate_work_unit_context(
        &self,
        task: &Task,
        work_unit_id: Option<&str>,
    ) -> Result<()> {
        let Some(work_unit_id) = work_unit_id else {
            return Ok(());
        };
        let work_unit_task_id = db::WorkUnitRepo::get_task_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| not_found("work_unit", work_unit_id.to_owned()))?;
        if work_unit_task_id != task.id {
            return Err(not_found("work_unit", work_unit_id.to_owned()));
        }
        db::WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .filter(|work_unit| work_unit.task_id == task.id)
            .ok_or_else(|| not_found("work_unit", work_unit_id.to_owned()))?;
        Ok(())
    }

    async fn validate_artifact_links(&self, task_id: &str, ids: &[String]) -> Result<()> {
        let mut unique = HashSet::new();
        for id in ids {
            if !unique.insert(id) {
                return Err(invalid(
                    "Artifact relationships cannot contain duplicate ids",
                ));
            }
            let artifact_task_id = CollaborationRepo::get_artifact_task_id(&*self.db, id)
                .await?
                .ok_or_else(|| not_found("artifact", id.clone()))?;
            if artifact_task_id != task_id {
                return Err(not_found("artifact", id.clone()));
            }
            CollaborationRepo::get_artifact(&*self.db, id)
                .await?
                .ok_or_else(|| not_found("artifact", id.clone()))?;
        }
        Ok(())
    }

    async fn validate_proposal_target(&self, task: &Task, target: &ProposalTarget) -> Result<()> {
        match target.kind {
            ProposalTargetKind::Task if target.id != task.id => {
                Err(not_found("task", target.id.clone()))
            }
            ProposalTargetKind::Task => Ok(()),
            ProposalTargetKind::Execution => {
                let execution_task_id = ExecutionRepo::get_task_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("execution", target.id.clone()))?;
                if execution_task_id != task.id {
                    return Err(not_found("execution", target.id.clone()));
                }
                let execution = ExecutionRepo::get_by_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("execution", target.id.clone()))?;
                if execution.task_id != task.id {
                    return Err(not_found("execution", target.id.clone()));
                }
                Ok(())
            }
            ProposalTargetKind::Workspace => {
                let workspace_task_id = WorkspaceRepo::get_task_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("workspace", target.id.clone()))?;
                if workspace_task_id != task.id {
                    return Err(not_found("workspace", target.id.clone()));
                }
                let workspace = WorkspaceRepo::get_by_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("workspace", target.id.clone()))?;
                if workspace.task_id != task.id {
                    return Err(not_found("workspace", target.id.clone()));
                }
                Ok(())
            }
            ProposalTargetKind::WorkUnit => {
                let work_unit_task_id = db::WorkUnitRepo::get_task_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("work_unit", target.id.clone()))?;
                if work_unit_task_id != task.id {
                    return Err(not_found("work_unit", target.id.clone()));
                }
                db::WorkUnitRepo::get_by_id(&*self.db, &target.id)
                    .await?
                    .filter(|work_unit| work_unit.task_id == task.id)
                    .ok_or_else(|| not_found("work_unit", target.id.clone()))?;
                Ok(())
            }
        }
    }

    async fn authorize_handoff_transition(
        &self,
        handoff: &Handoff,
        actor: &ActorRef,
        next: HandoffStatus,
    ) -> Result<()> {
        if next == HandoffStatus::Cancelled {
            return if handoff.created_by == *actor {
                Ok(())
            } else {
                Err(not_found("handoff", handoff.id.clone()))
            };
        }
        let authorized = match &handoff.target {
            CollaborationTarget::Actor(target) => target == actor,
            CollaborationTarget::Role(role_id) => {
                let role = TaskRoleRepo::get_by_id(&*self.db, role_id)
                    .await?
                    .ok_or_else(|| not_found("handoff", handoff.id.clone()))?;
                role.task_id == handoff.task_id
                    && self
                        .has_membership(&handoff.task_id, actor, Some(role_id))
                        .await?
            }
            CollaborationTarget::Task => true,
        };
        if authorized
            && matches!(
                next,
                HandoffStatus::Accepted | HandoffStatus::Declined | HandoffStatus::Completed
            )
        {
            Ok(())
        } else {
            Err(not_found("handoff", handoff.id.clone()))
        }
    }

    async fn require_membership(
        &self,
        task_id: &str,
        actor: &ActorRef,
        role_id: Option<&str>,
    ) -> Result<()> {
        if self.has_membership(task_id, actor, role_id).await? {
            Ok(())
        } else {
            Err(ServiceError::AuthorizationDenied {
                message: "Actor does not hold the addressed TaskRole".to_owned(),
            })
        }
    }

    async fn has_membership(
        &self,
        task_id: &str,
        actor: &ActorRef,
        role_id: Option<&str>,
    ) -> Result<bool> {
        let members = RoleMembershipRepo::list_by_task(&*self.db, task_id, false).await?;
        Ok(members.into_iter().any(|(role, membership)| {
            role_id.is_none_or(|id| role.id == id)
                && membership.actor_kind == actor.kind()
                && membership.actor_id == actor.id()
                && membership.status == RoleMembershipStatus::Active
        }))
    }

    async fn has_role_name_membership(
        &self,
        task_id: &str,
        actor: &ActorRef,
        role_name: &str,
    ) -> Result<bool> {
        let members = RoleMembershipRepo::list_by_task(&*self.db, task_id, false).await?;
        Ok(members.into_iter().any(|(role, membership)| {
            role.role == role_name
                && membership.actor_kind == actor.kind()
                && membership.actor_id == actor.id()
                && membership.status == RoleMembershipStatus::Active
        }))
    }

    async fn event_from_source(
        &self,
        source: &CollaborationActorSource,
        scope: EventScope<'_>,
        actor: &ActorRef,
        payload: serde_json::Value,
        now: &str,
    ) -> Result<CreateDomainEvent> {
        let mut correlation_id = new_uuid_v4();
        let mut causation_id = None;
        let mut causation_depth = 0;
        if let CollaborationActorSource::Execution(execution_id) = source {
            let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                .await?
                .ok_or_else(|| not_found("execution", execution_id.clone()))?;
            if execution.task_id != scope.task_id || execution.actor_ref() != Some(actor.clone()) {
                return Err(ServiceError::AuthorizationDenied {
                    message: "collaboration event source does not match its exact Task Actor"
                        .to_owned(),
                });
            }
            if execution.role == "orchestrator"
                && execution.purpose == Some(ExecutionPurpose::Orchestrate)
            {
                let start_event = DomainEventRepo::get_event_by_dedupe(
                    &*self.db,
                    &format!("execution.started:{}", execution.id),
                )
                .await?
                .filter(|event| {
                    event.entity_type == "execution"
                        && event.entity_id == execution.id
                        && event.scope_type == "task"
                        && event.scope_id == scope.task_id
                })
                .ok_or_else(|| {
                    invalid("orchestrator action has no durable Execution start event")
                })?;
                let depth = start_event.causation_depth.saturating_add(1);
                if depth > 16 {
                    return Err(invalid("orchestrator action exceeds causation depth limit"));
                }
                correlation_id = start_event.correlation_id;
                causation_id = Some(start_event.id);
                causation_depth = depth;
            }
        }
        Ok(CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: scope.event_type.to_owned(),
            entity_type: scope.entity_type.to_owned(),
            entity_id: scope.entity_id.to_owned(),
            actor_type: actor.kind().to_string(),
            actor_id: Some(actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: scope.task_id.to_owned(),
            correlation_id,
            causation_id,
            causation_depth,
            dedupe_key: None,
            payload_json: payload.to_string(),
            created_at: now.to_owned(),
        })
    }
}

fn validate_artifact_storage(
    storage_kind: ArtifactStorageKind,
    content: Option<&str>,
    content_ref: Option<&str>,
) -> Result<()> {
    match (storage_kind, content.is_some(), content_ref.is_some()) {
        (ArtifactStorageKind::Inline, true, false) => Ok(()),
        (ArtifactStorageKind::External, false, true)
            if content_ref.is_some_and(|value| !value.trim().is_empty()) =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "Artifact storage requires exactly one of inline content or external content_ref",
        )),
    }
}

fn message_matches(
    existing: &db::Message,
    task_id: &str,
    sender: &ActorRef,
    input: &CreateMessageInput,
) -> bool {
    existing.task_id == task_id
        && existing.sender == *sender
        && existing.target == input.target
        && existing.work_unit_id == input.work_unit_id
        && existing.body == input.body
        && existing.artifact_ids == input.artifact_ids
}

fn message_matches_record(existing: &db::Message, input: &CreateMessage) -> bool {
    existing.id == input.id
        && existing.task_id == input.task_id
        && existing.sender == input.sender
        && existing.target == input.target
        && existing.work_unit_id == input.work_unit_id
        && existing.body == input.body
        && existing.artifact_ids == input.artifact_ids
}

fn handoff_matches_record(existing: &Handoff, input: &CreateHandoff) -> bool {
    existing.id == input.id
        && existing.task_id == input.task_id
        && existing.created_by == input.created_by
        && existing.source_role_id == input.source_role_id
        && existing.target == input.target
        && existing.work_unit_id == input.work_unit_id
        && existing.intent == input.intent
        && existing.parent_execution_id == input.parent_execution_id
        && existing.expected_policy_ref == input.expected_policy_ref
        && existing.status == HandoffStatus::Pending
        && existing.artifact_ids == input.artifact_ids
}

fn proposal_matches_record(existing: &Proposal, input: &CreateProposal) -> bool {
    existing.id == input.id
        && existing.task_id == input.task_id
        && existing.proposer == input.proposer
        && existing.target == input.target
        && existing.action == input.action
        && existing.reason == input.reason
        && existing.target_version == input.target_version
        && existing.target_digest == input.target_digest
        && existing.required_policy_ref == input.required_policy_ref
        && existing.required_policy_version == input.required_policy_version
        && existing.required_policy_digest == input.required_policy_digest
        && existing.supersedes_proposal_id == input.supersedes_proposal_id
        && existing.status == ProposalStatus::Open
        && existing.artifact_ids == input.artifact_ids
}

fn not_found(entity: &'static str, id: String) -> ServiceError {
    ServiceError::NotFound { entity, id }
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOperation {
        message: message.into(),
    }
}
