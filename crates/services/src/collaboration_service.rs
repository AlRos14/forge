use std::{collections::HashSet, sync::Arc};

use db::{
    new_uuid_v4, now_rfc3339, ActorRef, AgentRepo, Artifact, ArtifactKind, ArtifactStorageKind,
    CollaborationRepo, CollaborationTarget, CreateArtifact, CreateDecision, CreateDomainEvent,
    CreateHandoff, CreateMessage, CreateProposal, Decision, DecisionOutcome, ExecutionRepo,
    Handoff, HandoffIntent, HandoffStatus, Page, PageRequest, Project, ProjectMemberRepo,
    ProjectRepo, Proposal, ProposalStatus, ProposalTarget, ProposalTargetKind, RoleMembershipRepo,
    RoleMembershipStatus, SqliteDb, Task, TaskRepo, TaskRoleRepo, UserRepo, WorkspaceRepo,
};
use events::EventBus;

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
    pub body: String,
    pub artifact_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CreateHandoffInput {
    pub task_id: String,
    pub source_role_id: Option<String>,
    pub target: CollaborationTarget,
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
        let execution = ExecutionRepo::get_by_id(&*self.db, &input.producer_execution_id)
            .await?
            .ok_or_else(|| not_found("execution", input.producer_execution_id.clone()))?;
        if execution.task_id != input.task_id || execution.actor_ref().as_ref() != Some(&actor) {
            return Err(not_found("execution", input.producer_execution_id));
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
        let event = self.event(
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
                "digest": input.digest,
            }),
            &now,
        );
        let write = CollaborationRepo::create_artifact(
            &*self.db,
            CreateArtifact {
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
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_artifact(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<Artifact> {
        let record = CollaborationRepo::get_artifact(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("artifact", id.to_owned()))?;
        self.authorize_source(&record.task_id, &source)
            .await
            .map_err(|_| not_found("artifact", id.to_owned()))?;
        Ok(record)
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
        let (task, sender) = self.authorize_source(&input.task_id, &source).await?;
        if input.body.trim().is_empty() {
            return Err(invalid("Message body must not be empty"));
        }
        self.validate_target(&task, &input.target).await?;
        self.validate_artifact_links(&task.id, &input.artifact_ids)
            .await?;
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
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
            }),
            &now,
        );
        let write = CollaborationRepo::create_message(
            &*self.db,
            CreateMessage {
                id,
                task_id: task.id,
                sender,
                target: input.target,
                body: input.body,
                artifact_ids: input.artifact_ids,
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_message(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<db::Message> {
        let record = CollaborationRepo::get_message(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("message", id.to_owned()))?;
        self.authorize_source(&record.task_id, &source)
            .await
            .map_err(|_| not_found("message", id.to_owned()))?;
        Ok(record)
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
        let (task, created_by) = self.authorize_source(&input.task_id, &source).await?;
        self.validate_target(&task, &input.target).await?;
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
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
            EventScope {
                event_type: "handoff.created",
                entity_type: "handoff",
                entity_id: &id,
                task_id: &task.id,
            },
            &created_by,
            serde_json::json!({
                "handoff_id": id,
                "task_id": task.id,
                "intent": input.intent.to_string(),
                "target_kind": input.target.kind().to_string(),
            }),
            &now,
        );
        let write = CollaborationRepo::create_handoff(
            &*self.db,
            CreateHandoff {
                id,
                task_id: task.id,
                created_by,
                source_role_id,
                target: input.target,
                intent: input.intent,
                parent_execution_id,
                expected_policy_ref: input.expected_policy_ref,
                artifact_ids: input.artifact_ids,
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_handoff(&self, id: &str, source: CollaborationActorSource) -> Result<Handoff> {
        let record = CollaborationRepo::get_handoff(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("handoff", id.to_owned()))?;
        self.authorize_source(&record.task_id, &source)
            .await
            .map_err(|_| not_found("handoff", id.to_owned()))?;
        Ok(record)
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
        let existing = CollaborationRepo::get_handoff(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("handoff", id.to_owned()))?;
        let (_, actor) = self.authorize_source(&existing.task_id, &source).await?;
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
        let event = self.event(
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
        );
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
        let (task, proposer) = self.authorize_source(&input.task_id, &source).await?;
        if input.action.trim().is_empty() {
            return Err(invalid("Proposal action must not be empty"));
        }
        self.validate_proposal_target(&task, &input.target).await?;
        if let Some(prior_id) = input.supersedes_proposal_id.as_deref() {
            let prior = CollaborationRepo::get_proposal(&*self.db, prior_id)
                .await?
                .ok_or_else(|| not_found("proposal", prior_id.to_owned()))?;
            if prior.task_id != task.id || prior.status != ProposalStatus::Superseded {
                return Err(invalid(
                    "superseded Proposal must be in the same Task and have a supersede Decision",
                ));
            }
        }
        self.validate_artifact_links(&task.id, &input.artifact_ids)
            .await?;
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
            EventScope {
                event_type: "proposal.created",
                entity_type: "proposal",
                entity_id: &id,
                task_id: &task.id,
            },
            &proposer,
            serde_json::json!({
                "proposal_id": id,
                "task_id": task.id,
                "target_kind": input.target.kind.to_string(),
                "action": input.action,
            }),
            &now,
        );
        let write = CollaborationRepo::create_proposal(
            &*self.db,
            CreateProposal {
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
                created_at: now,
            },
            event,
        )
        .await?;
        self.domain_events.publish_committed(&write.event);
        Ok(write.record)
    }

    pub async fn get_proposal(
        &self,
        id: &str,
        source: CollaborationActorSource,
    ) -> Result<Proposal> {
        let record = CollaborationRepo::get_proposal(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("proposal", id.to_owned()))?;
        self.authorize_source(&record.task_id, &source)
            .await
            .map_err(|_| not_found("proposal", id.to_owned()))?;
        Ok(record)
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
        let existing = CollaborationRepo::get_proposal(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("proposal", id.to_owned()))?;
        let (_, actor) = self.authorize_source(&existing.task_id, &source).await?;
        if existing.proposer != actor {
            return Err(not_found("proposal", id.to_owned()));
        }
        let now = now_rfc3339();
        let event = self.event(
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
        );
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
        let proposal = CollaborationRepo::get_proposal(&*self.db, &input.proposal_id)
            .await?
            .ok_or_else(|| not_found("proposal", input.proposal_id.clone()))?;
        if proposal.task_id != input.task_id
            || proposal.content_version != input.proposal_version
            || proposal.status != ProposalStatus::Open
        {
            return Err(invalid(
                "Decision Proposal is missing, stale, cross-Task, or already resolved",
            ));
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
        let initiating_actor = actors[0].clone();
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
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
        );
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
        let record = CollaborationRepo::get_decision(&*self.db, id)
            .await?
            .ok_or_else(|| not_found("decision", id.to_owned()))?;
        self.authorize_source(&record.task_id, &source)
            .await
            .map_err(|_| not_found("decision", id.to_owned()))?;
        Ok(record)
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

    async fn authorize_source(
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
                        if AgentRepo::get_by_id(&*self.db, agent_id).await?.is_none() {
                            return Err(invalid("Execution ActorRef no longer exists"));
                        }
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
                    }
                }
                actor
            }
        };
        Ok((task, actor))
    }

    async fn authorize_human_project(&self, project: &Project, user_id: &str) -> Result<()> {
        if UserRepo::get_user_by_id(&*self.db, user_id)
            .await?
            .is_none()
        {
            return Err(not_found("project", project.id.clone()));
        }
        let member = ProjectMemberRepo::get_member(&*self.db, &project.id, user_id)
            .await?
            .is_some();
        // This matches the current local-first Project access contract.
        if project.owner_id.is_none() || project.owner_id.as_deref() == Some(user_id) || member {
            Ok(())
        } else {
            Err(not_found("project", project.id.clone()))
        }
    }

    async fn validate_target(&self, task: &Task, target: &CollaborationTarget) -> Result<()> {
        match target {
            CollaborationTarget::Task => Ok(()),
            CollaborationTarget::Role(role_id) => {
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
                    if AgentRepo::get_by_id(&*self.db, agent_id).await?.is_none()
                        || !self.has_membership(&task.id, actor, None).await?
                    {
                        return Err(not_found("actor", agent_id.clone()));
                    }
                    Ok(())
                }
            },
        }
    }

    async fn validate_artifact_links(&self, task_id: &str, ids: &[String]) -> Result<()> {
        let mut unique = HashSet::new();
        for id in ids {
            if !unique.insert(id) {
                return Err(invalid(
                    "Artifact relationships cannot contain duplicate ids",
                ));
            }
            let artifact = CollaborationRepo::get_artifact(&*self.db, id)
                .await?
                .ok_or_else(|| not_found("artifact", id.clone()))?;
            if artifact.task_id != task_id {
                return Err(not_found("artifact", id.clone()));
            }
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
                let execution = ExecutionRepo::get_by_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("execution", target.id.clone()))?;
                if execution.task_id != task.id {
                    return Err(not_found("execution", target.id.clone()));
                }
                Ok(())
            }
            ProposalTargetKind::Workspace => {
                let workspace = WorkspaceRepo::get_by_id(&*self.db, &target.id)
                    .await?
                    .ok_or_else(|| not_found("workspace", target.id.clone()))?;
                if workspace.task_id != task.id {
                    return Err(not_found("workspace", target.id.clone()));
                }
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

    fn event(
        &self,
        scope: EventScope<'_>,
        actor: &ActorRef,
        payload: serde_json::Value,
        now: &str,
    ) -> CreateDomainEvent {
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: scope.event_type.to_owned(),
            entity_type: scope.entity_type.to_owned(),
            entity_id: scope.entity_id.to_owned(),
            actor_type: actor.kind().to_string(),
            actor_id: Some(actor.id().to_owned()),
            scope_type: "task".to_owned(),
            scope_id: scope.task_id.to_owned(),
            correlation_id: new_uuid_v4(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: payload.to_string(),
            created_at: now.to_owned(),
        }
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

fn not_found(entity: &'static str, id: String) -> ServiceError {
    ServiceError::NotFound { entity, id }
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOperation {
        message: message.into(),
    }
}
