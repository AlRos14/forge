//! PR6 durable event classifier and wake dispatcher.
//!
//! `EventBus` is only a latency hint. Every admission is based on a claimed
//! SQLite `domain_event`, and every automatic dispatch has an exact durable
//! TaskRole, Actor, wake row, and Execution attempt.

use std::{collections::HashSet, sync::Arc, time::Duration as StdDuration};

use chrono::{DateTime, Duration, Utc};
use db::{
    new_uuid_v4, now_rfc3339, ActorKind, ActorRef, AgentRepo, ClaimDomainEvents, CollaborationRepo,
    CollaborationTarget, CompleteDomainEvent, CoordinationMode, CreateOrchestratorWake,
    DomainEvent, DomainEventRepo, ExecutionPurpose, ExecutionRepo, ExecutionStatus,
    OrchestratorWake, OrchestratorWakeExecution, OrchestratorWakeRepo, OrchestratorWakeState,
    PageRequest, ProposalTarget, ProposalTargetKind, ReviewRepo, ReviewStatus, RoleMembership,
    RoleMembershipRepo, RoleMembershipStatus, SortBy, SortOrder, TaskRepo, TaskRole, TaskRoleRepo,
    WorkUnit, WorkUnitRepo, WorkUnitStatus, WorkUnitWorkspaceRepo, WorkspaceRepo,
};
use events::{EventBus, ForgeEvent};
use serde_json::{json, Value};
use sha2::Digest;
use tokio::{sync::watch, task::JoinHandle, time::MissedTickBehavior};
use uuid::Uuid;

use crate::{
    agent_capacity::has_running_execution_capacity,
    agent_service::{compute_effective_status, EffectiveStatus},
    collaboration_service::{
        CollaborationActorSource, CollaborationService, CreateHandoffInput, CreateMessageInput,
        CreateProposalInput,
    },
    task_service::TaskService,
    Result, ServiceError,
};

use db::HandoffIntent;
use serde::{Deserialize, Serialize};

const CONSUMER_NAME: &str = "task-orchestrator-wakes";
const EVENT_LEASE_SECONDS: i64 = 60;
const WAKE_LEASE_SECONDS: i64 = 60;
const FALLBACK_POLL: StdDuration = StdDuration::from_secs(5);
const RETRY_DELAY: Duration = Duration::seconds(15);
const MAX_AUTOMATIC_ATTEMPTS: i64 = 3;
const MAX_CONTEXT_ROWS: i64 = 30;
const MAX_CONTEXT_BYTES: usize = 512 * 1024;
const MAX_ACTION_RESPONSE_BYTES: usize = 64 * 1024;
const POLICY_REF: &str = "forge.orchestrator.wake-policy";
const POLICY_VERSION: i64 = 1;
const POLICY_TEXT: &str = "v1: exact-task-role; active-members-only; mode-aware targeting; read-only Codex CLI; context<=524288B; response<=65536B; actions<=16; work_units<=4; title<=512B; scope<=4096B; role<=128B; message<=8192B; proposal rationale<=8192B; protected actions only as proposals; no decision side effects";
const MAX_POLICY_ACTIONS: usize = 16;
const MAX_POLICY_WORK_UNITS: usize = 4;

fn current_policy_digest() -> String {
    hex::encode(sha2::Sha256::digest(POLICY_TEXT.as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrchestratorRun {
    pub claimed_events: usize,
    pub admitted_wakes: usize,
    pub processed_events: usize,
    pub dispatched_wakes: usize,
    pub retried_wakes: usize,
}

#[derive(Clone)]
pub struct OrchestratorRuntime {
    db: Arc<db::SqliteDb>,
    event_bus: Arc<EventBus>,
    task_service: Arc<TaskService>,
    consumer_name: String,
    lease_owner: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WakeSignal {
    work_unit_id: Option<String>,
    target: WakeTarget,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct OrchestratorResponse {
    actions: Vec<OrchestratorAction>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum OrchestratorAction {
    Message {
        target: ActionTarget,
        work_unit_id: Option<String>,
        body: String,
    },
    Handoff {
        target: ActionTarget,
        work_unit_id: Option<String>,
        intent: HandoffIntent,
    },
    CreateWorkUnit {
        title: String,
        scope: String,
        role: String,
        parent_work_unit_id: Option<String>,
        assigned_actor: Option<ActionActorRef>,
        requires_integration: bool,
    },
    Proposal {
        target: ProposalTarget,
        action: String,
        reason: String,
        target_version: Option<i64>,
        target_digest: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
enum OrchestratorActionKind {
    Message,
    Handoff,
    CreateWorkUnit,
    Proposal,
}

/// The only TaskRole policy understood by PR6. An empty object deliberately
/// retains the PR6 defaults. Unknown keys and schema versions are rejected by
/// this decoder so they cannot silently weaken dispatch or action checks.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct TaskRoleOrchestratorPolicy {
    schema_version: u32,
    automatic_orchestration: bool,
    allowed_actions: Option<Vec<OrchestratorActionKind>>,
    max_actions_per_execution: usize,
    max_work_unit_creations_per_execution: usize,
}

impl Default for TaskRoleOrchestratorPolicy {
    fn default() -> Self {
        Self {
            schema_version: 1,
            automatic_orchestration: true,
            allowed_actions: None,
            max_actions_per_execution: MAX_POLICY_ACTIONS,
            max_work_unit_creations_per_execution: MAX_POLICY_WORK_UNITS,
        }
    }
}

impl TaskRoleOrchestratorPolicy {
    pub(crate) fn parse(policy_json: &str) -> std::result::Result<Self, String> {
        let policy = serde_json::from_str::<Self>(policy_json)
            .map_err(|error| format!("TaskRole policy is not supported by PR6: {error}"))?;
        if policy.schema_version != 1 {
            return Err(format!(
                "TaskRole policy schema version {} is not supported by PR6",
                policy.schema_version
            ));
        }
        if policy.max_actions_per_execution > MAX_POLICY_ACTIONS {
            return Err(format!(
                "TaskRole policy max_actions_per_execution exceeds {MAX_POLICY_ACTIONS}"
            ));
        }
        if policy.max_work_unit_creations_per_execution > MAX_POLICY_WORK_UNITS {
            return Err(format!(
                "TaskRole policy max_work_unit_creations_per_execution exceeds {MAX_POLICY_WORK_UNITS}"
            ));
        }
        if let Some(actions) = policy.allowed_actions.as_ref() {
            let mut unique = HashSet::with_capacity(actions.len());
            if actions.iter().any(|action| !unique.insert(*action)) {
                return Err("TaskRole policy allowed_actions contains duplicates".to_owned());
            }
        }
        Ok(policy)
    }

    fn permits(&self, action: &OrchestratorAction) -> bool {
        self.allowed_actions
            .as_ref()
            .is_none_or(|allowed| allowed.contains(&action.policy_kind()))
    }

    fn action_limit(&self) -> usize {
        self.max_actions_per_execution
            .min(MAX_ACTION_RESPONSE_ACTIONS)
    }

    fn work_unit_limit(&self) -> usize {
        self.max_work_unit_creations_per_execution
            .min(MAX_POLICY_WORK_UNITS)
    }

    fn context_value(&self) -> Value {
        json!({
            "schema_version": self.schema_version,
            "automatic_orchestration": self.automatic_orchestration,
            "allowed_actions": self.allowed_actions.as_ref(),
            "max_actions_per_execution": self.max_actions_per_execution,
            "max_work_unit_creations_per_execution": self.max_work_unit_creations_per_execution,
        })
    }

    pub(crate) fn permits_automatic_orchestration(&self) -> bool {
        self.automatic_orchestration
    }
}

const MAX_ACTION_RESPONSE_ACTIONS: usize = MAX_POLICY_ACTIONS;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ActionActorRef {
    actor_kind: String,
    actor_id: String,
}

impl ActionActorRef {
    fn into_domain(self) -> Result<ActorRef> {
        if self.actor_id.trim().is_empty() {
            return Err(invalid_action("assigned Actor id must not be empty"));
        }
        match self.actor_kind.as_str() {
            "human" => Ok(ActorRef::Human(self.actor_id)),
            "agent" => Ok(ActorRef::Agent(self.actor_id)),
            _ => Err(invalid_action("assigned Actor kind is unsupported")),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ActionTarget {
    Task,
    Actor {
        actor_kind: String,
        actor_id: String,
    },
    Role {
        task_role_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WakeTarget {
    Task,
    Actor(ActorRef),
    Role(String),
}

impl OrchestratorRuntime {
    pub fn new(
        db: Arc<db::SqliteDb>,
        event_bus: Arc<EventBus>,
        task_service: Arc<TaskService>,
    ) -> Self {
        Self {
            db,
            event_bus,
            task_service,
            consumer_name: CONSUMER_NAME.to_owned(),
            lease_owner: Uuid::new_v4().to_string(),
        }
    }

    /// EventBus wakes the loop quickly, while the periodic durable scan covers
    /// restart, lag, and events committed before this subscriber existed.
    pub fn start(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut events = self.event_bus.subscribe();
            let mut fallback = tokio::time::interval(FALLBACK_POLL);
            fallback.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() { break; }
                    }
                    _ = fallback.tick() => self.run_logged().await,
                    hinted = events.recv() => match hinted {
                        Ok(event) if is_domain_event_hint(&event) => self.run_logged().await,
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Closed) => {},
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "orchestrator event hint lagged; durable scan remains authoritative");
                            self.run_logged().await;
                        }
                    }
                }
            }
        })
    }

    async fn run_logged(&self) {
        if let Err(error) = self.run_once(100).await {
            tracing::warn!(consumer = %self.consumer_name, %error, "durable orchestrator run failed");
        }
    }

    pub async fn run_once(&self, limit: i64) -> Result<OrchestratorRun> {
        let now = now_rfc3339();
        let events = DomainEventRepo::claim_event_batch(
            &*self.db,
            ClaimDomainEvents {
                consumer_name: self.consumer_name.clone(),
                lease_owner: self.lease_owner.clone(),
                now: now.clone(),
                leased_until: add_seconds(&now, EVENT_LEASE_SECONDS),
                limit: limit.clamp(1, 100),
            },
        )
        .await?;
        let claimed_events = events.len();
        let mut run = OrchestratorRun {
            claimed_events,
            admitted_wakes: 0,
            processed_events: 0,
            dispatched_wakes: 0,
            retried_wakes: 0,
        };

        for event in events {
            if self.reconcile_orchestrator_terminal(&event).await? {
                // Orchestrator lifecycle is handled only for its exact source
                // wake. It never enters generic event classification.
            } else if let Some(signal) = self.classify_event(&event).await? {
                run.admitted_wakes += self.admit_signal(&event, signal).await?;
            }
            let dedupe_key = event.dedupe_key.clone().unwrap_or_else(|| event.id.clone());
            DomainEventRepo::complete_claimed_event(
                &*self.db,
                CompleteDomainEvent {
                    consumer_name: self.consumer_name.clone(),
                    lease_owner: self.lease_owner.clone(),
                    event_sequence: event.sequence,
                    event_id: event.id,
                    dedupe_key,
                    completed_at: now_rfc3339(),
                },
            )
            .await?;
            run.processed_events += 1;
        }

        for _ in 0..16 {
            match self.dispatch_one().await? {
                DispatchOutcome::Idle => break,
                DispatchOutcome::Dispatched => run.dispatched_wakes += 1,
                DispatchOutcome::Retried => run.retried_wakes += 1,
            }
        }
        Ok(run)
    }

    async fn classify_event(&self, event: &DomainEvent) -> Result<Option<WakeSignal>> {
        if event.scope_type != "task" || !wake_depth_allowed(event.causation_depth) {
            return Ok(None);
        }
        let task_id = event.scope_id.as_str();
        let Some(task) = TaskRepo::get_by_id(&*self.db, task_id, false).await? else {
            return Ok(None);
        };
        if task.id != event.scope_id {
            return Ok(None);
        }
        let signal = match event.event_type.as_str() {
            "execution.started"
            | "execution.completed"
            | "execution.failed"
            | "execution.stalled" => {
                let Some(execution) = ExecutionRepo::get_by_id(&*self.db, &event.entity_id).await?
                else {
                    return Ok(None);
                };
                if matches!(
                    event.event_type.as_str(),
                    "execution.completed"
                        | "execution.failed"
                        | "execution.stalled"
                        | "execution.cancelled"
                ) && !terminal_event_matches_execution(&event.event_type, &execution.status)
                {
                    return Ok(None);
                }
                if !execution_lifecycle_is_orchestrator_wake(
                    &event.event_type,
                    &event.scope_id,
                    &execution.task_id,
                    execution.purpose,
                ) {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id: execution.work_unit_id,
                    target: WakeTarget::Task,
                }
            }
            "work_unit.completed"
            | "work_unit.cancelled"
            | "work_unit.dependency_added"
            | "work_unit.dependency_removed"
            | "work_unit.allocation_changed"
            | "work_unit.integration_succeeded"
            | "work_unit.integration_conflicted"
            | "work_unit.integration_failed" => {
                let (unit_id, unit_task) = if event.entity_type == "work_unit" {
                    let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, &event.entity_id).await?
                    else {
                        return Ok(None);
                    };
                    (unit.id, unit.task_id)
                } else if event.entity_type == "work_unit_integration" {
                    let payload = parse_payload(event);
                    let Some(unit_id) = payload.get("work_unit_id").and_then(Value::as_str) else {
                        return Ok(None);
                    };
                    let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, unit_id).await? else {
                        return Ok(None);
                    };
                    (unit.id, unit.task_id)
                } else {
                    return Ok(None);
                };
                if unit_task != task_id {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id: Some(unit_id),
                    target: WakeTarget::Task,
                }
            }
            "message.created" => {
                if event.entity_type != "message" {
                    return Ok(None);
                }
                let Some(message) =
                    CollaborationRepo::get_message(&*self.db, &event.entity_id).await?
                else {
                    return Ok(None);
                };
                if message.task_id != task_id {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id: message.work_unit_id,
                    target: target_from_collaboration(message.target),
                }
            }
            "handoff.created" | "handoff.status_changed" => {
                if event.entity_type != "handoff" {
                    return Ok(None);
                }
                let Some(handoff) =
                    CollaborationRepo::get_handoff(&*self.db, &event.entity_id).await?
                else {
                    return Ok(None);
                };
                if handoff.task_id != task_id {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id: handoff.work_unit_id,
                    target: target_from_collaboration(handoff.target),
                }
            }
            "proposal.created" => {
                if event.entity_type != "proposal" {
                    return Ok(None);
                }
                let Some(proposal) =
                    CollaborationRepo::get_proposal(&*self.db, &event.entity_id).await?
                else {
                    return Ok(None);
                };
                if proposal.task_id != task_id {
                    return Ok(None);
                }
                let work_unit_id = if proposal.target.kind == ProposalTargetKind::WorkUnit {
                    let Some(unit) =
                        WorkUnitRepo::get_by_id(&*self.db, &proposal.target.id).await?
                    else {
                        return Ok(None);
                    };
                    if unit.task_id != task_id {
                        return Ok(None);
                    }
                    Some(unit.id)
                } else {
                    None
                };
                WakeSignal {
                    work_unit_id,
                    target: WakeTarget::Task,
                }
            }
            "decision.recorded" => {
                if event.entity_type != "decision" {
                    return Ok(None);
                }
                let Some(decision) =
                    CollaborationRepo::get_decision(&*self.db, &event.entity_id).await?
                else {
                    return Ok(None);
                };
                if decision.task_id != task_id {
                    return Ok(None);
                }
                let Some(proposal) =
                    CollaborationRepo::get_proposal(&*self.db, &decision.proposal_id).await?
                else {
                    return Ok(None);
                };
                if proposal.task_id != task_id {
                    return Ok(None);
                }
                let work_unit_id = if proposal.target.kind == ProposalTargetKind::WorkUnit {
                    let Some(unit) =
                        WorkUnitRepo::get_by_id(&*self.db, &proposal.target.id).await?
                    else {
                        return Ok(None);
                    };
                    if unit.task_id != task_id {
                        return Ok(None);
                    }
                    Some(unit.id)
                } else {
                    None
                };
                WakeSignal {
                    work_unit_id,
                    target: WakeTarget::Actor(proposal.proposer),
                }
            }
            "review.status_changed" => {
                if event.entity_type != "review" {
                    return Ok(None);
                }
                let Some(review) = ReviewRepo::get_by_id(&*self.db, &event.entity_id).await? else {
                    return Ok(None);
                };
                if review.task_id != task_id || review.status != ReviewStatus::Failed {
                    return Ok(None);
                }
                let Some(execution) =
                    ExecutionRepo::get_by_id(&*self.db, &review.execution_id).await?
                else {
                    return Ok(None);
                };
                if execution.task_id != task_id {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id: execution.work_unit_id,
                    target: WakeTarget::Task,
                }
            }
            "task.transitioned" | "task.status_changed" => {
                if event.entity_type != "task" || event.entity_id != task.id {
                    return Ok(None);
                }
                if event.event_type == "task.status_changed" {
                    let payload = parse_payload(event);
                    if payload
                        .get("from_status")
                        .zip(payload.get("to_status"))
                        .is_some_and(|(from, to)| from == to)
                    {
                        return Ok(None);
                    }
                }
                WakeSignal {
                    work_unit_id: None,
                    target: WakeTarget::Task,
                }
            }
            "task.blocked" | "task.unblocked" | "task.failed" => {
                if event.entity_type != "task" || event.entity_id != task.id {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id: None,
                    target: WakeTarget::Task,
                }
            }
            "orchestrator.task_role_changed" => {
                if event.entity_type != "task_role" {
                    return Ok(None);
                }
                let Some(role) = TaskRoleRepo::get_by_id(&*self.db, &event.entity_id).await? else {
                    return Ok(None);
                };
                if role.task_id != task_id || role.role != "orchestrator" {
                    return Ok(None);
                }
                let payload = parse_payload(event);
                if payload.get("task_role_version").and_then(Value::as_i64) != Some(role.version) {
                    return Ok(None);
                }
                let work_unit_id = payload
                    .get("work_unit_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                let target = match (
                    payload.get("actor_kind").and_then(Value::as_str),
                    payload.get("actor_id").and_then(Value::as_str),
                ) {
                    (Some("human"), Some(actor_id)) => {
                        WakeTarget::Actor(ActorRef::Human(actor_id.to_owned()))
                    }
                    (Some("agent"), Some(actor_id)) => {
                        WakeTarget::Actor(ActorRef::Agent(actor_id.to_owned()))
                    }
                    (None, None) => WakeTarget::Role(role.id.clone()),
                    _ => return Ok(None),
                };
                if role.coordination_mode == Some(CoordinationMode::Partitioned) {
                    let Some(work_unit_id) = work_unit_id.as_deref() else {
                        return Ok(None);
                    };
                    let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, work_unit_id).await? else {
                        return Ok(None);
                    };
                    let WakeTarget::Actor(actor) = &target else {
                        return Ok(None);
                    };
                    if unit.task_id != task_id
                        || unit.role != "orchestrator"
                        || unit.assigned_actor.as_ref() != Some(actor)
                    {
                        return Ok(None);
                    }
                } else if work_unit_id.is_some() {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id,
                    target,
                }
            }
            "orchestrator.membership_changed" => {
                if event.entity_type != "role_membership" {
                    return Ok(None);
                }
                let Some(membership) = RoleMembershipRepo::get(&*self.db, &event.entity_id).await?
                else {
                    return Ok(None);
                };
                let payload = parse_payload(event);
                if payload.get("membership_version").and_then(Value::as_i64)
                    != Some(membership.version)
                {
                    return Ok(None);
                }
                let Some(role) =
                    TaskRoleRepo::get_by_id(&*self.db, &membership.task_role_id).await?
                else {
                    return Ok(None);
                };
                if role.task_id != task_id || role.role != "orchestrator" {
                    return Ok(None);
                }
                let target = if membership.status == RoleMembershipStatus::Active {
                    WakeTarget::Actor(membership.actor_ref())
                } else {
                    WakeTarget::Role(role.id)
                };
                WakeSignal {
                    work_unit_id: None,
                    target,
                }
            }
            "orchestrator.bootstrap_reconciled" => {
                if event.entity_type != "task_role" {
                    return Ok(None);
                }
                let Some(role) = TaskRoleRepo::get_by_id(&*self.db, &event.entity_id).await? else {
                    return Ok(None);
                };
                let payload = parse_payload(event);
                if role.task_id != task_id
                    || role.role != "orchestrator"
                    || payload.get("task_role_id").and_then(Value::as_str) != Some(role.id.as_str())
                    || payload.get("bootstrap_version").and_then(Value::as_i64) != Some(1)
                {
                    return Ok(None);
                }
                let current_mode = role.coordination_mode.map(|mode| mode.to_string());
                let event_mode = payload
                    .get("coordination_mode")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if current_mode != event_mode {
                    return Ok(None);
                }
                let Some(actor_kind) = payload.get("actor_kind").and_then(Value::as_str) else {
                    return Ok(None);
                };
                let Some(actor_id) = payload.get("actor_id").and_then(Value::as_str) else {
                    return Ok(None);
                };
                let actor = match actor_kind {
                    "human" => ActorRef::Human(actor_id.to_owned()),
                    "agent" => ActorRef::Agent(actor_id.to_owned()),
                    _ => return Ok(None),
                };
                let work_unit_id = payload
                    .get("work_unit_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if role.coordination_mode == Some(CoordinationMode::Partitioned) {
                    let Some(work_unit_id) = work_unit_id.as_deref() else {
                        return Ok(None);
                    };
                    let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, work_unit_id).await? else {
                        return Ok(None);
                    };
                    if unit.task_id != task_id
                        || unit.role != "orchestrator"
                        || unit.assigned_actor.as_ref() != Some(&actor)
                    {
                        return Ok(None);
                    }
                } else if work_unit_id.is_some() {
                    return Ok(None);
                }
                WakeSignal {
                    work_unit_id,
                    target: WakeTarget::Actor(actor),
                }
            }
            _ => return Ok(None),
        };
        Ok(Some(signal))
    }

    async fn admit_signal(&self, event: &DomainEvent, signal: WakeSignal) -> Result<usize> {
        let task_id = event.scope_id.as_str();
        let Some(role) =
            TaskRoleRepo::get_by_task_and_role(&*self.db, task_id, "orchestrator").await?
        else {
            return Ok(0);
        };
        let memberships = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
        let active: Vec<_> = memberships
            .into_iter()
            .filter(|membership| membership.status == RoleMembershipStatus::Active)
            .collect();
        let unit = match signal.work_unit_id.as_deref() {
            Some(id) => {
                let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, id).await? else {
                    return Ok(0);
                };
                if unit.task_id != task_id {
                    return Ok(0);
                }
                Some(unit)
            }
            None => None,
        };
        let targets = select_orchestrator_targets(
            role.coordination_mode,
            &role.id,
            &active,
            unit.as_ref(),
            &signal.target,
        );
        let now = now_rfc3339();
        let policy_digest = current_policy_digest();
        let role_policy = TaskRoleOrchestratorPolicy::parse(&role.policy_json).ok();
        let mut admitted = 0;
        for membership in targets {
            if event_is_authored_by_member(event, membership) {
                continue;
            }
            if membership.actor_kind == ActorKind::Agent
                && role_policy
                    .as_ref()
                    .is_some_and(|policy| !policy.permits_automatic_orchestration())
            {
                continue;
            }
            let id = new_uuid_v4();
            if OrchestratorWakeRepo::admit_orchestrator_wake(
                &*self.db,
                CreateOrchestratorWake {
                    id,
                    event_id: event.id.clone(),
                    event_sequence: event.sequence,
                    task_id: task_id.to_owned(),
                    task_role_id: role.id.clone(),
                    coordination_mode: role.coordination_mode,
                    actor_kind: membership.actor_kind,
                    actor_id: membership.actor_id.clone(),
                    work_unit_id: signal.work_unit_id.clone(),
                    correlation_id: event.correlation_id.clone(),
                    causation_id: Some(event.id.clone()),
                    causation_depth: event.causation_depth,
                    policy_ref: POLICY_REF.to_owned(),
                    policy_version: POLICY_VERSION,
                    policy_digest: policy_digest.clone(),
                    task_role_version: role.version,
                    task_role_policy_json: role.policy_json.clone(),
                    available_at: now.clone(),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await?
            {
                admitted += 1;
            }
        }
        Ok(admitted)
    }

    async fn dispatch_one(&self) -> Result<DispatchOutcome> {
        let now = now_rfc3339();
        let Some(wake) = OrchestratorWakeRepo::claim_orchestrator_wake(
            &*self.db,
            db::ClaimOrchestratorWake {
                lease_owner: self.lease_owner.clone(),
                now: now.clone(),
                leased_until: add_seconds(&now, WAKE_LEASE_SECONDS),
            },
        )
        .await?
        else {
            return Ok(DispatchOutcome::Idle);
        };
        if !wake_uses_current_policy(&wake) {
            self.fail_wake(
                &wake,
                "durable wake policy reference or digest is unsupported",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let role = TaskRoleRepo::get_by_id(&*self.db, &wake.task_role_id).await?;
        let Some(role) =
            role.filter(|role| role.task_id == wake.task_id && role.role == "orchestrator")
        else {
            self.fail_wake(&wake, "canonical TaskRole was removed or changed")
                .await?;
            return Ok(DispatchOutcome::Retried);
        };
        if role.version != wake.task_role_version || role.policy_json != wake.task_role_policy_json
        {
            self.fail_wake(
                &wake,
                "TaskRole version or policy changed after wake admission",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let role_policy = match TaskRoleOrchestratorPolicy::parse(&role.policy_json) {
            Ok(policy) => policy,
            Err(reason) => {
                self.fail_wake(&wake, &reason).await?;
                return Ok(DispatchOutcome::Retried);
            }
        };
        if wake.actor_kind == ActorKind::Agent && !role_policy.permits_automatic_orchestration() {
            self.fail_wake(
                &wake,
                "TaskRole policy disables automatic Agent orchestration",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        if role.coordination_mode != wake.coordination_mode {
            self.fail_wake(
                &wake,
                "orchestrator coordination mode changed after wake admission",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let memberships = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
        if !memberships.iter().any(|member| {
            member.status == RoleMembershipStatus::Active
                && member.actor_kind == wake.actor_kind
                && member.actor_id == wake.actor_id
        }) {
            self.fail_wake(
                &wake,
                "target Actor is no longer an active orchestrator member",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let source_event = DomainEventRepo::get_event(&*self.db, &wake.event_id).await?;
        let Some(source_event) = source_event.filter(|event| {
            event.sequence == wake.event_sequence
                && event.scope_type == "task"
                && event.scope_id == wake.task_id
        }) else {
            self.fail_wake(&wake, "durable wake cause no longer matches its Task scope")
                .await?;
            return Ok(DispatchOutcome::Retried);
        };
        let Some(signal) = self.classify_event(&source_event).await? else {
            self.fail_wake(&wake, "durable wake cause is no longer a supported signal")
                .await?;
            return Ok(DispatchOutcome::Retried);
        };
        if signal.work_unit_id != wake.work_unit_id {
            self.fail_wake(&wake, "durable wake WorkUnit context changed")
                .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let unit = match wake.work_unit_id.as_deref() {
            Some(id) => WorkUnitRepo::get_by_id(&*self.db, id).await?,
            None => None,
        };
        let selected = select_orchestrator_targets(
            role.coordination_mode,
            &role.id,
            &memberships,
            unit.as_ref(),
            &signal.target,
        );
        let selected_actor = selected
            .iter()
            .any(|member| member.actor_kind == wake.actor_kind && member.actor_id == wake.actor_id);
        let self_event = memberships.iter().any(|member| {
            member.status == RoleMembershipStatus::Active
                && member.actor_kind == wake.actor_kind
                && member.actor_id == wake.actor_id
                && event_is_authored_by_member(&source_event, member)
        });
        if !selected_actor || self_event {
            self.fail_wake(
                &wake,
                "wake Actor is no longer selected by the exact targeting policy",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        if wake.actor_kind == ActorKind::Human {
            OrchestratorWakeRepo::transition_orchestrator_wake(
                &*self.db,
                db::TransitionOrchestratorWake {
                    id: wake.id,
                    expected_state: Some(OrchestratorWakeState::Leased),
                    lease_owner: Some(self.lease_owner.clone()),
                    state: OrchestratorWakeState::AwaitingHuman,
                    available_at: None,
                    current_attempt: None,
                    last_error: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await?;
            return Ok(DispatchOutcome::Dispatched);
        }
        let agent = match AgentRepo::get_by_id(&*self.db, &wake.actor_id).await? {
            Some(agent) => agent,
            None => {
                self.fail_wake(&wake, "active Agent membership has no Agent record")
                    .await?;
                return Ok(DispatchOutcome::Retried);
            }
        };
        if compute_effective_status(&self.db, &agent).await? != EffectiveStatus::Active
            || !has_running_execution_capacity(&self.db, &agent).await?
        {
            self.defer_wake(&wake, "Agent is temporarily unavailable or at capacity")
                .await?;
            return Ok(DispatchOutcome::Retried);
        }
        if agent.backend_kind == "native" || agent.executor_type != "codex" {
            self.fail_wake(
                &wake,
                "automatic PR6 dispatch requires a Codex CLI Agent with native read-only sandbox capability",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }

        let context = match self
            .build_context(&wake, &role, &memberships, &role_policy)
            .await
        {
            Ok(context) => context,
            Err(error) => {
                let reason = format!("orchestrator context load failed: {error}");
                match error {
                    ServiceError::InvalidOperation { .. }
                    | ServiceError::AuthorizationDenied { .. }
                    | ServiceError::NotFound { .. }
                    | ServiceError::Conflict(_) => self.fail_wake(&wake, &reason).await?,
                    transient => {
                        self.defer_wake(
                            &wake,
                            &format!("orchestrator context load deferred: {transient}"),
                        )
                        .await?
                    }
                }
                return Ok(DispatchOutcome::Retried);
            }
        };
        let context_bytes = serde_json::to_vec(&context).map_err(|error| {
            ServiceError::invalid_operation(format!("serialize orchestrator context: {error}"))
        })?;
        if context_bytes.len() > MAX_CONTEXT_BYTES {
            self.fail_wake(
                &wake,
                "orchestrator context exceeds the 512 KiB policy limit",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let prompt = render_orchestrator_prompt(&wake, &context);
        let reserved = OrchestratorWakeRepo::reserve_orchestrator_wake_execution(
            &*self.db,
            db::ReserveOrchestratorWakeExecution {
                wake_id: wake.id.clone(),
                lease_owner: self.lease_owner.clone(),
                execution_id: new_uuid_v4(),
                now: now_rfc3339(),
            },
        )
        .await?;
        match reserved.state.as_str() {
            "start_requested" | "running" | "uncertain" => {
                self.make_uncertain(&wake, &reserved, "prior harness start outcome is ambiguous")
                    .await?;
                return Ok(DispatchOutcome::Retried);
            }
            "reserved" => {}
            _ => {
                self.fail_wake(&wake, "unknown durable Execution attempt state")
                    .await?;
                return Ok(DispatchOutcome::Retried);
            }
        }
        let Some(wake) = OrchestratorWakeRepo::get_orchestrator_wake(&*self.db, &wake.id).await?
        else {
            return Err(ServiceError::invalid_operation(
                "durable orchestrator wake disappeared after Execution reservation",
            ));
        };
        if wake.state != OrchestratorWakeState::Leased
            || wake.lease_owner.as_deref() != Some(self.lease_owner.as_str())
            || wake.current_attempt != Some(reserved.attempt_number)
        {
            self.make_uncertain(
                &wake,
                &reserved,
                "wake lease or reserved attempt changed after reservation",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        let execution = match ExecutionRepo::get_by_id(&*self.db, &reserved.execution_id).await? {
            Some(execution) => execution,
            None => match self
                .task_service
                .dispatch_orchestrator_execution(&wake, &reserved, &self.lease_owner, prompt)
                .await
            {
                Ok(execution) => execution,
                Err(error) => {
                    let reason = error.to_string();
                    match error {
                        ServiceError::InvalidOperation { .. }
                        | ServiceError::AuthorizationDenied { .. }
                        | ServiceError::NotFound { .. }
                        | ServiceError::Conflict(_) => {
                            self.fail_wake(
                                &wake,
                                &format!("Execution admission rejected: {reason}"),
                            )
                            .await?;
                        }
                        transient => {
                            self.defer_wake(
                                &wake,
                                &format!("Execution admission deferred: {transient}"),
                            )
                            .await?;
                        }
                    }
                    return Ok(DispatchOutcome::Retried);
                }
            },
        };
        if execution.status != ExecutionStatus::Running
            || execution.task_id != wake.task_id
            || execution.workspace_id.is_some()
            || execution.role != "orchestrator"
            || execution.purpose != Some(ExecutionPurpose::Orchestrate)
            || execution.actor_ref() != Some(ActorRef::Agent(wake.actor_id.clone()))
        {
            self.make_uncertain(
                &wake,
                &reserved,
                "reserved Execution is incompatible with its wake",
            )
            .await?;
            return Ok(DispatchOutcome::Retried);
        }
        if !OrchestratorWakeRepo::transition_orchestrator_wake_execution(
            &*self.db,
            db::TransitionOrchestratorWakeExecution {
                wake_id: wake.id.clone(),
                attempt_number: reserved.attempt_number,
                execution_id: reserved.execution_id.clone(),
                lease_owner: Some(self.lease_owner.clone()),
                expected_state: Some("reserved".to_owned()),
                state: "start_requested".to_owned(),
                last_error: None,
                updated_at: now_rfc3339(),
            },
        )
        .await?
        {
            self.defer_wake(&wake, "wake lease changed before Harness Start")
                .await?;
            return Ok(DispatchOutcome::Retried);
        }
        match self.task_service.start_execution(&execution.id).await {
            Ok(_) => {
                OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                    &*self.db,
                    db::TransitionOrchestratorWakeExecution {
                        wake_id: wake.id.clone(),
                        attempt_number: reserved.attempt_number,
                        execution_id: reserved.execution_id.clone(),
                        lease_owner: None,
                        expected_state: Some("start_requested".to_owned()),
                        state: "running".to_owned(),
                        last_error: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
                OrchestratorWakeRepo::transition_orchestrator_wake(
                    &*self.db,
                    db::TransitionOrchestratorWake {
                        id: wake.id,
                        expected_state: Some(OrchestratorWakeState::Leased),
                        lease_owner: Some(self.lease_owner.clone()),
                        state: OrchestratorWakeState::Running,
                        available_at: None,
                        current_attempt: None,
                        last_error: None,
                        updated_at: now_rfc3339(),
                    },
                )
                .await?;
                Ok(DispatchOutcome::Dispatched)
            }
            Err(error) => {
                self.make_uncertain(
                    &wake,
                    &reserved,
                    &format!("Harness start outcome is ambiguous: {error}"),
                )
                .await?;
                Ok(DispatchOutcome::Retried)
            }
        }
    }

    async fn build_context(
        &self,
        wake: &OrchestratorWake,
        role: &TaskRole,
        memberships: &[RoleMembership],
        role_policy: &TaskRoleOrchestratorPolicy,
    ) -> Result<Value> {
        let task = TaskRepo::get_by_id(&*self.db, &wake.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", wake.task_id.clone()))?;
        let event = DomainEventRepo::get_event(&*self.db, &wake.event_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("domain_event", wake.event_id.clone()))?;
        let units = if let Some(id) = wake.work_unit_id.as_deref() {
            WorkUnitRepo::get_by_id(&*self.db, id)
                .await?
                .filter(|unit| unit.task_id == task.id)
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            WorkUnitRepo::list_by_task(&*self.db, &task.id)
                .await?
                .into_iter()
                .take(MAX_CONTEXT_ROWS as usize)
                .collect()
        };
        let mut unit_context = Vec::new();
        for unit in units {
            let dependencies = WorkUnitRepo::list_dependencies(&*self.db, &unit.id).await?;
            let integrations = WorkUnitRepo::list_integrations(&*self.db, &unit.id).await?;
            let dependencies_satisfied = dependencies.iter().all(|dependency| dependency.satisfied);
            let active_execution_ids =
                WorkUnitRepo::list_active_execution_ids(&*self.db, &unit.id).await?;
            let scope_ready = unit.status == WorkUnitStatus::Open
                && dependencies_satisfied
                && active_execution_ids.is_empty();
            let dependencies_json = dependencies
                .iter()
                .map(|dependency| {
                    json!({
                        "work_unit_id": dependency.work_unit_id,
                        "depends_on_work_unit_id": dependency.depends_on_work_unit_id,
                        "satisfied": dependency.satisfied,
                    })
                })
                .collect::<Vec<_>>();
            let integrations_json = integrations
                .iter()
                .map(|integration| {
                    json!({
                        "id": integration.id,
                        "execution_id": integration.execution_id,
                        "source_sha": integration.source_sha,
                        "target_before_sha": integration.target_before_sha,
                        "target_after_sha": integration.target_after_sha,
                        "outcome": integration.outcome.to_string(),
                        "conflict_metadata": integration.conflict_metadata_json.as_ref().map(|value| truncate(value, 4096)),
                        "finished_at": integration.finished_at,
                    })
                })
                .collect::<Vec<_>>();
            unit_context.push(json!({
                "id": unit.id,
                "task_id": unit.task_id,
                "parent_work_unit_id": unit.parent_work_unit_id,
                "title": truncate(&unit.title, 512),
                "scope": truncate(&unit.scope, 4096),
                "status": unit.status.to_string(),
                "role": unit.role,
                "assigned_actor": unit.assigned_actor,
                "requires_integration": unit.requires_integration,
                "dependencies_satisfied": dependencies_satisfied,
                "active_execution_ids": active_execution_ids,
                "scope_ready": scope_ready,
                "dependencies": dependencies_json,
                "integrations": integrations_json,
            }));
        }
        let page = recent_page(MAX_CONTEXT_ROWS);
        let task_integration_workspace = WorkspaceRepo::get_by_task_id(&*self.db, &task.id).await?;
        let work_unit_workspace = match wake.work_unit_id.as_deref() {
            Some(id) => WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, id).await?,
            None => None,
        };
        let mut executions = ExecutionRepo::list_by_task(&*self.db, &task.id, page.clone())
            .await?
            .items;
        let mut artifacts = CollaborationRepo::list_artifacts(&*self.db, &task.id, page.clone())
            .await?
            .items;
        let mut messages = CollaborationRepo::list_messages(&*self.db, &task.id, page.clone())
            .await?
            .items;
        let mut handoffs = CollaborationRepo::list_handoffs(&*self.db, &task.id, page.clone())
            .await?
            .items;
        let mut proposals = CollaborationRepo::list_proposals(&*self.db, &task.id, page.clone())
            .await?
            .items;
        let mut decisions = CollaborationRepo::list_decisions(&*self.db, &task.id, page)
            .await?
            .items;
        if let Some(work_unit_id) = wake.work_unit_id.as_deref() {
            executions.retain(|execution| {
                work_unit_context_matches(Some(work_unit_id), execution.work_unit_id.as_deref())
            });
            messages.retain(|message| {
                work_unit_context_matches(Some(work_unit_id), message.work_unit_id.as_deref())
            });
            handoffs.retain(|handoff| {
                work_unit_context_matches(Some(work_unit_id), handoff.work_unit_id.as_deref())
            });
            proposals.retain(|proposal| {
                work_unit_proposal_matches(
                    Some(work_unit_id),
                    proposal.target.kind,
                    &proposal.target.id,
                )
            });
            let proposal_ids = proposals
                .iter()
                .map(|proposal| proposal.id.as_str())
                .collect::<HashSet<_>>();
            decisions.retain(|decision| proposal_ids.contains(decision.proposal_id.as_str()));
            let mut linked_artifact_ids = HashSet::new();
            for message in &messages {
                linked_artifact_ids.extend(message.artifact_ids.iter().map(String::as_str));
            }
            for handoff in &handoffs {
                linked_artifact_ids.extend(handoff.artifact_ids.iter().map(String::as_str));
            }
            for proposal in &proposals {
                linked_artifact_ids.extend(proposal.artifact_ids.iter().map(String::as_str));
            }
            let execution_ids = executions
                .iter()
                .map(|execution| execution.id.as_str())
                .collect::<HashSet<_>>();
            artifacts.retain(|artifact| {
                linked_artifact_ids.contains(artifact.id.as_str())
                    || execution_ids.contains(artifact.producer_execution_id.as_str())
            });
        }
        let event_payload = parse_payload(&event);
        Ok(json!({
            "task": {
                "id": task.id,
                "project_id": task.project_id,
                "title": truncate(&task.title, 512),
                "description": task.description.as_deref().map(|value| truncate(value, 16 * 1024)),
                "status": task.status,
                "task_type": task.task_type,
                "version": task.version,
            },
            "task_role": {
                "id": role.id,
                "role": role.role,
                "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
                "policy_ref": POLICY_REF,
                "policy_version": POLICY_VERSION,
                "policy_digest": wake.policy_digest,
                "version": wake.task_role_version,
                "policy_json": wake.task_role_policy_json,
                "policy_digest_for_task_role": task_role_policy_digest(&wake.task_role_policy_json),
                "pr6_policy": role_policy.context_value(),
            },
            "active_memberships": memberships.iter()
                .filter(|membership| membership.status == RoleMembershipStatus::Active)
                .map(|membership| json!({"actor_kind": membership.actor_kind.to_string(), "actor_id": membership.actor_id}))
                .collect::<Vec<_>>(),
            "work_units": unit_context,
            "workspace_state": {
                "task_integration": task_integration_workspace.map(|workspace| json!({
                    "workspace_id": workspace.id,
                    "status": format!("{:?}", workspace.status),
                    "branch": workspace.branch,
                    "before_sha": workspace.before_sha,
                })),
                "work_unit": work_unit_workspace.map(|workspace| json!({
                    "workspace_id": workspace.id,
                    "status": format!("{:?}", workspace.status),
                    "branch": workspace.branch,
                    "before_sha": workspace.before_sha,
                })),
            },
            "executions": executions.iter().map(|execution| json!({
                "id": execution.id,
                "actor_kind": execution.actor_kind.map(|kind| kind.to_string()),
                "actor_id": execution.actor_id,
                "role": execution.role,
                "purpose": execution.purpose.as_ref().map(|purpose| purpose.to_string()),
                "status": execution.status.to_string(),
                "work_unit_id": execution.work_unit_id,
                "summary": execution.summary.as_deref().map(|value| truncate(value, 4096)),
                "error": execution.error.as_deref().map(|value| truncate(value, 4096)),
            })).collect::<Vec<_>>(),
            "artifacts": artifacts.iter().map(|artifact| json!({
                "id": artifact.id,
                "kind": artifact.kind.to_string(),
                "digest": artifact.digest,
                "metadata": truncate(&artifact.metadata_json, 4096),
                "content": artifact.content.as_deref().map(|content| truncate(content, 4096)),
                "producer_execution_id": artifact.producer_execution_id,
            })).collect::<Vec<_>>(),
            "messages": messages.iter().map(|message| json!({
                "id": message.id,
                "sender": message.sender,
                "target": message.target,
                "work_unit_id": message.work_unit_id,
                "body": truncate(&message.body, 4096),
                "artifact_ids": message.artifact_ids,
            })).collect::<Vec<_>>(),
            "handoffs": handoffs.iter().map(|handoff| json!({
                "id": handoff.id,
                "target": handoff.target,
                "work_unit_id": handoff.work_unit_id,
                "intent": handoff.intent.to_string(),
                "status": handoff.status.to_string(),
                "artifact_ids": handoff.artifact_ids,
            })).collect::<Vec<_>>(),
            "proposals": proposals.iter().map(|proposal| json!({
                "id": proposal.id,
                "target": proposal.target,
                "action": truncate(&proposal.action, 1024),
                "reason": truncate(&proposal.reason, 2048),
                "target_version": proposal.target_version,
                "target_digest": proposal.target_digest,
                "required_policy_ref": proposal.required_policy_ref,
                "required_policy_version": proposal.required_policy_version,
                "required_policy_digest": proposal.required_policy_digest,
                "status": proposal.status.to_string(),
            })).collect::<Vec<_>>(),
            "decisions": decisions.iter().map(|decision| json!({
                "id": decision.id,
                "proposal_id": decision.proposal_id,
                "proposal_version": decision.proposal_version,
                "outcome": decision.outcome.to_string(),
                "rationale": truncate(&decision.rationale, 2048),
                "policy_ref": decision.policy_ref,
                "policy_version": decision.policy_version,
                "policy_digest": decision.policy_digest,
            })).collect::<Vec<_>>(),
            "wake": {
                "id": wake.id,
                "event_id": event.id,
                "event_sequence": event.sequence,
                "event_type": event.event_type,
                "event_entity_type": event.entity_type,
                "event_entity_id": event.entity_id,
                "scope_type": event.scope_type,
                "scope_id": event.scope_id,
                "correlation_id": event.correlation_id,
                "causation_id": event.causation_id,
                "causation_depth": event.causation_depth,
                "payload": event_payload,
                "work_unit_id": wake.work_unit_id,
            }
        }))
    }

    async fn reconcile_orchestrator_terminal(&self, event: &DomainEvent) -> Result<bool> {
        if event.entity_type != "execution"
            || event.scope_type != "task"
            || !matches!(
                event.event_type.as_str(),
                "execution.completed"
                    | "execution.failed"
                    | "execution.stalled"
                    | "execution.cancelled"
            )
        {
            return Ok(false);
        }
        let Some(execution) = ExecutionRepo::get_by_id(&*self.db, &event.entity_id).await? else {
            return Ok(false);
        };
        if execution.purpose != Some(ExecutionPurpose::Orchestrate)
            || execution.role != "orchestrator"
            || execution.task_id != event.scope_id
        {
            return Ok(false);
        }
        let event_status_matches =
            terminal_event_matches_execution(&event.event_type, &execution.status);
        if !event_status_matches {
            // This event belongs to an orchestrate Execution, so it must not
            // fall through to generic wake classification. Its payload is
            // only a signal; the current durable Execution status is authority.
            return Ok(true);
        }
        let Some((wake, attempt)) =
            OrchestratorWakeRepo::get_orchestrator_wake_by_execution(&*self.db, &execution.id)
                .await?
        else {
            return Ok(true);
        };
        // A crash after updating the wake but before completing the source
        // event receipt must not reset the retry delay or reverse a terminal
        // result when the event is replayed.
        if wake.state == OrchestratorWakeState::Pending && wake.current_attempt.is_none()
            || matches!(
                wake.state,
                OrchestratorWakeState::Completed | OrchestratorWakeState::Failed
            )
        {
            return Ok(true);
        }
        if wake.current_attempt != Some(attempt.attempt_number) {
            return Ok(true);
        }
        if !wake_uses_current_policy(&wake) {
            let now = now_rfc3339();
            OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                &*self.db,
                db::TransitionOrchestratorWakeExecution {
                    wake_id: wake.id.clone(),
                    attempt_number: attempt.attempt_number,
                    execution_id: execution.id.clone(),
                    lease_owner: None,
                    expected_state: Some(attempt.state.clone()),
                    state: "failed".to_owned(),
                    last_error: Some(Some(
                        "durable wake policy reference or digest is unsupported".to_owned(),
                    )),
                    updated_at: now.clone(),
                },
            )
            .await?;
            OrchestratorWakeRepo::transition_orchestrator_wake(
                &*self.db,
                db::TransitionOrchestratorWake {
                    id: wake.id.clone(),
                    expected_state: Some(wake.state.clone()),
                    lease_owner: None,
                    state: OrchestratorWakeState::Failed,
                    available_at: None,
                    current_attempt: None,
                    last_error: Some(Some(
                        "durable wake policy reference or digest is unsupported".to_owned(),
                    )),
                    updated_at: now,
                },
            )
            .await?;
            return Ok(true);
        }
        let Some(expected_running_state) = terminal_reconciliation_state(&wake, &attempt) else {
            return Ok(true);
        };
        let now = now_rfc3339();
        if event.event_type == "execution.completed" {
            if let Err(error) = self.apply_completed_actions(&execution, &wake).await {
                match error {
                    ServiceError::InvalidOperation { .. }
                    | ServiceError::AuthorizationDenied { .. }
                    | ServiceError::NotFound { .. }
                    | ServiceError::Conflict(_)
                    | ServiceError::Db(db::DbError::StaleOrchestratorAction) => {
                        OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                            &*self.db,
                            db::TransitionOrchestratorWakeExecution {
                                wake_id: wake.id.clone(),
                                attempt_number: attempt.attempt_number,
                                execution_id: execution.id.clone(),
                                lease_owner: None,
                                expected_state: Some(attempt.state.clone()),
                                state: "failed".to_owned(),
                                last_error: Some(Some(error.to_string())),
                                updated_at: now.clone(),
                            },
                        )
                        .await?;
                        OrchestratorWakeRepo::transition_orchestrator_wake(
                            &*self.db,
                            db::TransitionOrchestratorWake {
                                id: wake.id,
                                expected_state: Some(expected_running_state),
                                lease_owner: None,
                                state: OrchestratorWakeState::Failed,
                                available_at: None,
                                current_attempt: None,
                                last_error: Some(Some(error.to_string())),
                                updated_at: now,
                            },
                        )
                        .await?;
                        return Ok(true);
                    }
                    transient => return Err(transient),
                }
            }
            OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                &*self.db,
                db::TransitionOrchestratorWakeExecution {
                    wake_id: wake.id.clone(),
                    attempt_number: attempt.attempt_number,
                    execution_id: execution.id,
                    lease_owner: None,
                    expected_state: Some(attempt.state.clone()),
                    state: "completed".to_owned(),
                    last_error: None,
                    updated_at: now.clone(),
                },
            )
            .await?;
            OrchestratorWakeRepo::transition_orchestrator_wake(
                &*self.db,
                db::TransitionOrchestratorWake {
                    id: wake.id,
                    expected_state: Some(expected_running_state),
                    lease_owner: None,
                    state: OrchestratorWakeState::Completed,
                    available_at: None,
                    current_attempt: None,
                    last_error: None,
                    updated_at: now,
                },
            )
            .await?;
        } else if event.event_type != "execution.cancelled"
            && wake.attempt_count < MAX_AUTOMATIC_ATTEMPTS
            && wake.causation_depth < 16
        {
            let reason = if event.event_type == "execution.stalled" {
                "orchestrator Execution stalled"
            } else {
                "orchestrator Execution failed"
            };
            OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                &*self.db,
                db::TransitionOrchestratorWakeExecution {
                    wake_id: wake.id.clone(),
                    attempt_number: attempt.attempt_number,
                    execution_id: execution.id,
                    lease_owner: None,
                    expected_state: Some(attempt.state.clone()),
                    state: "failed".to_owned(),
                    last_error: Some(Some(reason.to_owned())),
                    updated_at: now.clone(),
                },
            )
            .await?;
            OrchestratorWakeRepo::transition_orchestrator_wake(
                &*self.db,
                db::TransitionOrchestratorWake {
                    id: wake.id,
                    expected_state: Some(expected_running_state),
                    lease_owner: None,
                    state: OrchestratorWakeState::Pending,
                    available_at: Some(add_duration(&now, RETRY_DELAY)),
                    current_attempt: Some(None),
                    last_error: Some(Some(reason.to_owned())),
                    updated_at: now,
                },
            )
            .await?;
        } else {
            let reason = match event.event_type.as_str() {
                "execution.cancelled" => {
                    "orchestrator Execution was cancelled; a new explicit collaboration wake is required"
                }
                "execution.stalled" => {
                    "orchestrator Execution stalled and retry limit was reached"
                }
                _ => "orchestrator Execution failed and retry limit was reached",
            };
            OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                &*self.db,
                db::TransitionOrchestratorWakeExecution {
                    wake_id: wake.id.clone(),
                    attempt_number: attempt.attempt_number,
                    execution_id: execution.id.clone(),
                    lease_owner: None,
                    expected_state: Some(attempt.state.clone()),
                    state: "failed".to_owned(),
                    last_error: Some(Some(reason.to_owned())),
                    updated_at: now.clone(),
                },
            )
            .await?;
            OrchestratorWakeRepo::transition_orchestrator_wake(
                &*self.db,
                db::TransitionOrchestratorWake {
                    id: wake.id,
                    expected_state: Some(expected_running_state),
                    lease_owner: None,
                    state: OrchestratorWakeState::Failed,
                    available_at: None,
                    current_attempt: Some(None),
                    last_error: Some(Some(reason.to_owned())),
                    updated_at: now,
                },
            )
            .await?;
        }
        Ok(true)
    }

    async fn apply_completed_actions(
        &self,
        execution: &db::Execution,
        wake: &OrchestratorWake,
    ) -> Result<()> {
        if !wake_uses_current_policy(wake) {
            return Err(invalid_action(
                "completed orchestrator wake has an unsupported policy reference or digest",
            ));
        }
        let role_policy = self.validate_current_role_policy(wake).await?;
        let response_text = execution.summary.as_deref().ok_or_else(|| {
            invalid_action("completed orchestrator Execution has no action response")
        })?;
        if response_text.len() > MAX_ACTION_RESPONSE_BYTES {
            return Err(invalid_action(
                "orchestrator response exceeds the 64 KiB action limit",
            ));
        }
        let response =
            serde_json::from_str::<OrchestratorResponse>(response_text).map_err(|_| {
                invalid_action("orchestrator response is not the required JSON action envelope")
            })?;
        validate_action_policy(&role_policy, &response.actions)?;
        let collaboration =
            CollaborationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let work_units = self.task_service.orchestrator_work_unit_service();
        for (index, action) in response.actions.into_iter().enumerate() {
            let current_policy = self.validate_current_role_policy(wake).await?;
            if !current_policy.permits(&action) {
                return Err(invalid_action(
                    "TaskRole policy no longer permits this orchestrator action",
                ));
            }
            let action_json = serde_json::to_vec(&action)
                .map_err(|_| invalid_action("orchestrator action cannot be serialized"))?;
            let action_digest = hex::encode(sha2::Sha256::digest(&action_json));
            let action_type = action.action_type();
            let now = now_rfc3339();
            let record = OrchestratorWakeRepo::reserve_orchestrator_action(
                &*self.db,
                db::ReserveOrchestratorAction {
                    execution_id: execution.id.clone(),
                    action_index: index as i64,
                    action_type: action_type.to_owned(),
                    action_digest,
                    result_id: new_uuid_v4(),
                    now: now.clone(),
                },
            )
            .await?;
            if record.state == "completed" {
                continue;
            }
            let current_policy = self.validate_current_role_policy(wake).await?;
            if !current_policy.permits(&action) {
                return Err(invalid_action(
                    "TaskRole policy changed before the orchestrator action was applied",
                ));
            }
            let source = CollaborationActorSource::Execution(execution.id.clone());
            match action {
                OrchestratorAction::Message {
                    target,
                    work_unit_id,
                    body,
                } => {
                    if !action_work_unit_matches(
                        wake.work_unit_id.as_deref(),
                        work_unit_id.as_deref(),
                    ) {
                        return Err(invalid_action(
                            "Message WorkUnit must remain inside the exact wake scope",
                        ));
                    }
                    if body.trim().is_empty()
                        || body.len() > 8192
                        || work_unit_id
                            .as_ref()
                            .is_some_and(|id| id.trim().is_empty() || id.len() > 128)
                    {
                        return Err(invalid_action("Message body must contain 1 to 8192 bytes"));
                    }
                    collaboration
                        .create_message_with_id(
                            source,
                            CreateMessageInput {
                                task_id: wake.task_id.clone(),
                                target: target.into_domain()?,
                                work_unit_id,
                                body,
                                artifact_ids: Vec::new(),
                            },
                            record.result_id.clone(),
                        )
                        .await?;
                }
                OrchestratorAction::Handoff {
                    target,
                    work_unit_id,
                    intent,
                } => {
                    if !action_work_unit_matches(
                        wake.work_unit_id.as_deref(),
                        work_unit_id.as_deref(),
                    ) {
                        return Err(invalid_action(
                            "Handoff WorkUnit must remain inside the exact wake scope",
                        ));
                    }
                    if work_unit_id
                        .as_ref()
                        .is_some_and(|id| id.trim().is_empty() || id.len() > 128)
                    {
                        return Err(invalid_action("Handoff WorkUnit id must be bounded"));
                    }
                    collaboration
                        .create_handoff_with_id(
                            source,
                            CreateHandoffInput {
                                task_id: wake.task_id.clone(),
                                source_role_id: None,
                                target: target.into_domain()?,
                                work_unit_id,
                                intent,
                                parent_execution_id: Some(execution.id.clone()),
                                expected_policy_ref: Some(wake.policy_ref.clone()),
                                artifact_ids: Vec::new(),
                            },
                            record.result_id.clone(),
                        )
                        .await?;
                }
                OrchestratorAction::CreateWorkUnit {
                    title,
                    scope,
                    role,
                    parent_work_unit_id,
                    assigned_actor,
                    requires_integration,
                } => {
                    if title.trim().is_empty()
                        || title.len() > 512
                        || scope.trim().is_empty()
                        || scope.len() > 4096
                        || role.trim().is_empty()
                        || role.len() > 128
                        || parent_work_unit_id
                            .as_ref()
                            .is_some_and(|id| id.trim().is_empty() || id.len() > 128)
                        || assigned_actor
                            .as_ref()
                            .is_some_and(|actor| actor.actor_id.len() > 128)
                        || wake
                            .work_unit_id
                            .as_deref()
                            .is_some_and(|id| parent_work_unit_id.as_deref() != Some(id))
                    {
                        return Err(invalid_action(
                            "WorkUnit creation requires bounded title, scope, role, parent, and Actor fields",
                        ));
                    }
                    work_units
                        .create_with_id(
                            source,
                            crate::CreateWorkUnitInput {
                                task_id: wake.task_id.clone(),
                                title,
                                scope,
                                role,
                                parent_work_unit_id,
                                assigned_actor: assigned_actor
                                    .map(ActionActorRef::into_domain)
                                    .transpose()?,
                                requires_integration,
                                provenance: Some(db::WorkUnitProvenance::Actor(ActorRef::Agent(
                                    wake.actor_id.clone(),
                                ))),
                            },
                            record.result_id.clone(),
                        )
                        .await?;
                }
                OrchestratorAction::Proposal {
                    target,
                    action,
                    reason,
                    target_version,
                    target_digest,
                } => {
                    if wake.work_unit_id.as_deref().is_some_and(|id| {
                        target.kind != ProposalTargetKind::WorkUnit || target.id != id
                    }) || target.id.trim().is_empty()
                        || target.id.len() > 128
                        || action.len() > 128
                        || target_digest
                            .as_ref()
                            .is_some_and(|digest| digest.len() > 128)
                        || target_version.is_some_and(|version| version < 1)
                        || !is_protected_action(&action)
                        || reason.trim().is_empty()
                        || reason.len() > 8192
                        || (target_version.is_none() && target_digest.is_none())
                    {
                        return Err(invalid_action(
                            "Proposal must name a protected action, bounded rationale, and target version or digest",
                        ));
                    }
                    collaboration
                        .create_proposal_with_id(
                            source,
                            CreateProposalInput {
                                task_id: wake.task_id.clone(),
                                target,
                                action,
                                reason,
                                target_version,
                                target_digest,
                                required_policy_ref: Some(wake.policy_ref.clone()),
                                required_policy_version: Some(wake.policy_version),
                                required_policy_digest: Some(wake.policy_digest.clone()),
                                supersedes_proposal_id: None,
                                artifact_ids: Vec::new(),
                            },
                            record.result_id.clone(),
                        )
                        .await?;
                }
            }
            OrchestratorWakeRepo::complete_orchestrator_action(
                &*self.db,
                &execution.id,
                index as i64,
                &now_rfc3339(),
            )
            .await?;
        }
        Ok(())
    }

    async fn validate_current_role_policy(
        &self,
        wake: &OrchestratorWake,
    ) -> Result<TaskRoleOrchestratorPolicy> {
        let Some(role) = TaskRoleRepo::get_by_id(&*self.db, &wake.task_role_id).await? else {
            return Err(invalid_action("orchestrator TaskRole no longer exists"));
        };
        if role.task_id != wake.task_id
            || role.role != "orchestrator"
            || role.coordination_mode != wake.coordination_mode
            || role.version != wake.task_role_version
            || role.policy_json != wake.task_role_policy_json
        {
            return Err(invalid_action(
                "TaskRole version, coordination, or policy changed after wake admission",
            ));
        }
        let memberships = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
        if !memberships.iter().any(|membership| {
            membership.status == RoleMembershipStatus::Active
                && membership.actor_kind == wake.actor_kind
                && membership.actor_id == wake.actor_id
        }) {
            return Err(invalid_action(
                "orchestrator Actor is no longer an active TaskRole member",
            ));
        }
        let policy =
            TaskRoleOrchestratorPolicy::parse(&role.policy_json).map_err(invalid_action)?;
        if wake.actor_kind == ActorKind::Agent && !policy.permits_automatic_orchestration() {
            return Err(invalid_action(
                "TaskRole policy disables automatic Agent orchestration",
            ));
        }
        Ok(policy)
    }

    async fn defer_wake(&self, wake: &OrchestratorWake, reason: &str) -> Result<()> {
        let now = now_rfc3339();
        OrchestratorWakeRepo::transition_orchestrator_wake(
            &*self.db,
            db::TransitionOrchestratorWake {
                id: wake.id.clone(),
                expected_state: Some(OrchestratorWakeState::Leased),
                lease_owner: Some(self.lease_owner.clone()),
                state: OrchestratorWakeState::Pending,
                available_at: Some(add_duration(&now, RETRY_DELAY)),
                current_attempt: None,
                last_error: Some(Some(reason.to_owned())),
                updated_at: now,
            },
        )
        .await?;
        Ok(())
    }

    async fn fail_wake(&self, wake: &OrchestratorWake, reason: &str) -> Result<()> {
        OrchestratorWakeRepo::transition_orchestrator_wake(
            &*self.db,
            db::TransitionOrchestratorWake {
                id: wake.id.clone(),
                expected_state: Some(OrchestratorWakeState::Leased),
                lease_owner: Some(self.lease_owner.clone()),
                state: OrchestratorWakeState::Failed,
                available_at: None,
                current_attempt: None,
                last_error: Some(Some(reason.to_owned())),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        Ok(())
    }

    async fn make_uncertain(
        &self,
        wake: &OrchestratorWake,
        attempt: &OrchestratorWakeExecution,
        reason: &str,
    ) -> Result<()> {
        let Some((current_wake, current_attempt)) =
            OrchestratorWakeRepo::get_orchestrator_wake_by_execution(
                &*self.db,
                &attempt.execution_id,
            )
            .await?
        else {
            return Ok(());
        };
        if current_wake.id != wake.id
            || current_wake.state != OrchestratorWakeState::Leased
            || current_wake.lease_owner.as_deref() != Some(self.lease_owner.as_str())
            || current_wake.current_attempt != Some(attempt.attempt_number)
            || matches!(current_attempt.state.as_str(), "completed" | "failed")
        {
            // A terminal event may have won the race while Harness Start was
            // returning. Never overwrite that durable outcome with uncertainty.
            return Ok(());
        }
        let changed = OrchestratorWakeRepo::transition_orchestrator_wake_execution(
            &*self.db,
            db::TransitionOrchestratorWakeExecution {
                wake_id: current_wake.id.clone(),
                attempt_number: current_attempt.attempt_number,
                execution_id: current_attempt.execution_id.clone(),
                lease_owner: Some(self.lease_owner.clone()),
                expected_state: Some(current_attempt.state),
                state: "uncertain".to_owned(),
                last_error: Some(Some(reason.to_owned())),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        if !changed {
            return Ok(());
        }
        OrchestratorWakeRepo::transition_orchestrator_wake(
            &*self.db,
            db::TransitionOrchestratorWake {
                id: current_wake.id,
                expected_state: Some(OrchestratorWakeState::Leased),
                lease_owner: Some(self.lease_owner.clone()),
                state: OrchestratorWakeState::Uncertain,
                available_at: None,
                current_attempt: None,
                last_error: Some(Some(reason.to_owned())),
                updated_at: now_rfc3339(),
            },
        )
        .await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DispatchOutcome {
    Idle,
    Dispatched,
    Retried,
}

impl OrchestratorAction {
    fn policy_kind(&self) -> OrchestratorActionKind {
        match self {
            Self::Message { .. } => OrchestratorActionKind::Message,
            Self::Handoff { .. } => OrchestratorActionKind::Handoff,
            Self::CreateWorkUnit { .. } => OrchestratorActionKind::CreateWorkUnit,
            Self::Proposal { .. } => OrchestratorActionKind::Proposal,
        }
    }

    fn action_type(&self) -> &'static str {
        match self {
            Self::Message { .. } => "message",
            Self::Handoff { .. } => "handoff",
            Self::CreateWorkUnit { .. } => "work_unit",
            Self::Proposal { .. } => "proposal",
        }
    }
}

impl ActionTarget {
    fn into_domain(self) -> Result<CollaborationTarget> {
        match self {
            Self::Task => Ok(CollaborationTarget::Task),
            Self::Role { task_role_id }
                if !task_role_id.trim().is_empty() && task_role_id.len() <= 128 =>
            {
                Ok(CollaborationTarget::Role(task_role_id))
            }
            Self::Role { .. } => Err(invalid_action(
                "Action target TaskRole id is empty or oversized",
            )),
            Self::Actor {
                actor_kind,
                actor_id,
            } if !actor_id.trim().is_empty() && actor_id.len() <= 128 => {
                let actor = match actor_kind.as_str() {
                    "human" => ActorRef::Human(actor_id),
                    "agent" => ActorRef::Agent(actor_id),
                    _ => return Err(invalid_action("Action target Actor kind is unsupported")),
                };
                Ok(CollaborationTarget::Actor(actor))
            }
            Self::Actor { .. } => Err(invalid_action(
                "Action target Actor id is empty or oversized",
            )),
        }
    }
}

fn work_unit_context_matches(
    expected_work_unit_id: Option<&str>,
    record_work_unit_id: Option<&str>,
) -> bool {
    expected_work_unit_id.is_none_or(|expected| record_work_unit_id == Some(expected))
}

fn wake_uses_current_policy(wake: &OrchestratorWake) -> bool {
    wake.policy_ref == POLICY_REF
        && wake.policy_version == POLICY_VERSION
        && wake.policy_digest == current_policy_digest()
}

fn task_role_policy_digest(policy_json: &str) -> String {
    hex::encode(sha2::Sha256::digest(policy_json.as_bytes()))
}

fn terminal_event_matches_execution(event_type: &str, status: &ExecutionStatus) -> bool {
    match event_type {
        "execution.completed" => status == &ExecutionStatus::Completed,
        "execution.failed" | "execution.stalled" => status == &ExecutionStatus::Failed,
        "execution.cancelled" => status == &ExecutionStatus::Cancelled,
        _ => false,
    }
}

fn terminal_reconciliation_state(
    wake: &OrchestratorWake,
    attempt: &OrchestratorWakeExecution,
) -> Option<OrchestratorWakeState> {
    match wake.state {
        OrchestratorWakeState::Running | OrchestratorWakeState::Uncertain => Some(wake.state),
        OrchestratorWakeState::Leased
            if matches!(
                attempt.state.as_str(),
                "start_requested" | "running" | "uncertain"
            ) =>
        {
            // An Execution may reach a durable terminal state while its
            // Harness Start caller still holds the wake lease. The terminal
            // event must win that race instead of being receipted and lost.
            Some(OrchestratorWakeState::Leased)
        }
        _ => None,
    }
}

fn work_unit_proposal_matches(
    expected_work_unit_id: Option<&str>,
    target_kind: ProposalTargetKind,
    target_id: &str,
) -> bool {
    expected_work_unit_id
        .is_none_or(|expected| target_kind == ProposalTargetKind::WorkUnit && target_id == expected)
}

fn action_work_unit_matches(
    expected_work_unit_id: Option<&str>,
    action_work_unit_id: Option<&str>,
) -> bool {
    expected_work_unit_id.is_none_or(|expected| action_work_unit_id == Some(expected))
}

fn is_protected_action(action: &str) -> bool {
    matches!(
        action,
        "stop" | "cancel" | "reassign" | "discard" | "invalidate" | "merge" | "override"
    )
}

fn invalid_action(message: impl Into<String>) -> ServiceError {
    ServiceError::InvalidOperation {
        message: message.into(),
    }
}

fn validate_action_policy(
    policy: &TaskRoleOrchestratorPolicy,
    actions: &[OrchestratorAction],
) -> Result<()> {
    if actions.len() > policy.action_limit() {
        return Err(invalid_action(
            "orchestrator response exceeds the TaskRole action limit",
        ));
    }
    if actions
        .iter()
        .filter(|action| matches!(action, OrchestratorAction::CreateWorkUnit { .. }))
        .count()
        > policy.work_unit_limit()
    {
        return Err(invalid_action(
            "orchestrator response exceeds the TaskRole WorkUnit creation limit",
        ));
    }
    if actions.iter().any(|action| !policy.permits(action)) {
        return Err(invalid_action(
            "orchestrator response contains an action denied by the TaskRole policy",
        ));
    }
    Ok(())
}

fn select_orchestrator_targets<'a>(
    mode: Option<CoordinationMode>,
    orchestrator_role_id: &str,
    members: &'a [RoleMembership],
    work_unit: Option<&WorkUnit>,
    target: &WakeTarget,
) -> Vec<&'a RoleMembership> {
    let exact_actor = |actor: &ActorRef| {
        members.iter().find(|membership| {
            membership.status == RoleMembershipStatus::Active
                && membership.actor_kind.to_string() == actor.kind().to_string()
                && membership.actor_id == actor.id()
        })
    };
    match target {
        WakeTarget::Actor(actor) => exact_actor(actor).into_iter().collect(),
        WakeTarget::Role(role_id) if role_id == orchestrator_role_id => match mode {
            Some(CoordinationMode::Collaborative) => members
                .iter()
                .filter(|member| member.status == RoleMembershipStatus::Active)
                .collect(),
            Some(CoordinationMode::Partitioned) => {
                if let Some(unit) = work_unit.filter(|unit| unit.role == "orchestrator") {
                    unit.assigned_actor
                        .as_ref()
                        .and_then(exact_actor)
                        .into_iter()
                        .collect()
                } else {
                    only_member(members)
                }
            }
            Some(CoordinationMode::Independent) | None => only_member(members),
        },
        WakeTarget::Role(_) => Vec::new(),
        WakeTarget::Task => match mode {
            Some(CoordinationMode::Collaborative) => members
                .iter()
                .filter(|member| member.status == RoleMembershipStatus::Active)
                .collect(),
            Some(CoordinationMode::Partitioned) => work_unit
                .filter(|unit| unit.role == "orchestrator")
                .and_then(|unit| unit.assigned_actor.as_ref())
                .and_then(exact_actor)
                .into_iter()
                .collect(),
            Some(CoordinationMode::Independent) | None => Vec::new(),
        },
    }
}

fn only_member(members: &[RoleMembership]) -> Vec<&RoleMembership> {
    let mut active = members
        .iter()
        .filter(|member| member.status == RoleMembershipStatus::Active);
    let Some(first) = active.next() else {
        return Vec::new();
    };
    if active.next().is_some() {
        Vec::new()
    } else {
        vec![first]
    }
}

fn target_from_collaboration(target: CollaborationTarget) -> WakeTarget {
    match target {
        CollaborationTarget::Actor(actor) => WakeTarget::Actor(actor),
        CollaborationTarget::Role(role) => WakeTarget::Role(role),
        CollaborationTarget::Task => WakeTarget::Task,
    }
}

fn is_domain_event_hint(event: &ForgeEvent) -> bool {
    event.event_type == "domain_event.committed"
}

fn wake_depth_allowed(depth: i64) -> bool {
    // A wake creates an Execution-start event and that Execution may emit
    // one collaboration event. Keep both increments inside the depth ceiling.
    (0..15).contains(&depth)
}

fn execution_lifecycle_is_orchestrator_wake(
    event_type: &str,
    event_task_id: &str,
    execution_task_id: &str,
    purpose: Option<ExecutionPurpose>,
) -> bool {
    matches!(
        event_type,
        "execution.started" | "execution.completed" | "execution.failed" | "execution.stalled"
    ) && event_task_id == execution_task_id
        && purpose != Some(ExecutionPurpose::Orchestrate)
}

fn event_is_authored_by_member(event: &DomainEvent, member: &RoleMembership) -> bool {
    event.actor_type == member.actor_kind.to_string()
        && event.actor_id.as_deref() == Some(member.actor_id.as_str())
}

fn parse_payload(event: &DomainEvent) -> Value {
    const MAX_EVENT_PAYLOAD_BYTES: usize = 16 * 1024;
    if event.payload_json.len() > MAX_EVENT_PAYLOAD_BYTES {
        return json!({
            "truncated": true,
            "byte_length": event.payload_json.len(),
            "sha256": hex::encode(sha2::Sha256::digest(event.payload_json.as_bytes())),
        });
    }
    serde_json::from_str(&event.payload_json).unwrap_or(Value::Null)
}

fn recent_page(limit: i64) -> PageRequest {
    PageRequest {
        cursor: None,
        limit,
        include_total: false,
        sort_by: SortBy::CreatedAt,
        sort_order: SortOrder::Desc,
    }
}

fn add_seconds(value: &str, seconds: i64) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| (time + Duration::seconds(seconds)).to_rfc3339())
        .unwrap_or_else(|_| (Utc::now() + Duration::seconds(seconds)).to_rfc3339())
}

fn add_duration(value: &str, duration: Duration) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|time| (time + duration).to_rfc3339())
        .unwrap_or_else(|_| (Utc::now() + duration).to_rfc3339())
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let result = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{result}[truncated]")
    } else {
        result
    }
}

fn render_orchestrator_prompt(wake: &OrchestratorWake, context: &Value) -> String {
    format!(
        "You are the Actor holding TaskRole `orchestrator` for this exact Task. Direct work through collaboration records. This execution has a read-only isolated context and must not modify repository files.\n\
         Return exactly one JSON object with an `actions` array. Supported action types are `message`, `handoff`, `create_work_unit`, and `proposal`. WorkUnit creation is limited to four bounded records per Execution and never starts work by itself. A WorkUnit-scoped wake may only reference that exact WorkUnit and may create a child under it. A Handoff expresses work intent and never changes RoleMembership. A Proposal never executes its action. Do not claim that stop, cancel, reassign, discard, invalidate, merge, or override occurred. If no action is needed, return `{{\"actions\":[]}}`.\n\
         Wake {} was caused by durable event {}. Runtime policy {} version {} digest {}; TaskRole policy version {} digest {}.\n\
         Task-scoped context follows as JSON:\n{}",
        wake.id,
        wake.event_id,
        wake.policy_ref,
        wake.policy_version,
        wake.policy_digest,
        wake.task_role_version,
        task_role_policy_digest(&wake.task_role_policy_json),
        context
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use db::{
        CreateProject, CreateRoleMembership, CreateTask, CreateTaskRole, ProjectRepo,
        RoleMembershipRepo, TaskRepo, TaskRoleRepo, User, UserRepo, WorkUnitRepo,
    };
    use sqlx::Row;

    async fn test_user(database: &Arc<db::SqliteDb>, name: &str) -> String {
        let now = now_rfc3339();
        let id = new_uuid_v4();
        UserRepo::create_user(
            &**database,
            &User {
                id: id.clone(),
                email: format!("{id}@example.test"),
                password_hash: "test-only".to_owned(),
                display_name: Some(name.to_owned()),
                is_admin: false,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("Human Actor creates");
        id
    }

    async fn test_project(database: &Arc<db::SqliteDb>, owner_id: &str) -> String {
        let now = now_rfc3339();
        let id = new_uuid_v4();
        ProjectRepo::create(
            &**database,
            CreateProject {
                id: id.clone(),
                name: "PR6 durable wake fixture".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(owner_id.to_owned()),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("project creates");
        id
    }

    async fn test_task(database: &Arc<db::SqliteDb>, project_id: &str, title: &str) -> String {
        let now = now_rfc3339();
        let id = new_uuid_v4();
        TaskRepo::create(
            &**database,
            CreateTask {
                id: id.clone(),
                project_id: project_id.to_owned(),
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: title.to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                plan: None,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("task creates");
        id
    }

    async fn test_orchestrator_role(
        database: &Arc<db::SqliteDb>,
        task_id: &str,
        mode: Option<CoordinationMode>,
        policy_json: &str,
    ) -> String {
        let now = now_rfc3339();
        let id = new_uuid_v4();
        TaskRoleRepo::create(
            &**database,
            CreateTaskRole {
                id: id.clone(),
                task_id: task_id.to_owned(),
                role: "orchestrator".to_owned(),
                coordination_mode: mode,
                policy_json: policy_json.to_owned(),
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("canonical orchestrator TaskRole creates");
        id
    }

    async fn test_membership(
        database: &Arc<db::SqliteDb>,
        role_id: &str,
        actor_id: &str,
        status: RoleMembershipStatus,
    ) -> String {
        let now = now_rfc3339();
        let id = new_uuid_v4();
        RoleMembershipRepo::add(
            &**database,
            CreateRoleMembership {
                id: id.clone(),
                task_role_id: role_id.to_owned(),
                actor_kind: ActorKind::Human,
                actor_id: actor_id.to_owned(),
                status,
                created_at: now.clone(),
                updated_at: now,
            },
        )
        .await
        .expect("TaskRole membership creates");
        id
    }

    async fn set_orchestrator_cursor_to_head(database: &Arc<db::SqliteDb>) {
        let now = now_rfc3339();
        let head: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM domain_event")
            .fetch_one(database.pool())
            .await
            .expect("domain-event head loads");
        sqlx::query(
            "INSERT INTO event_consumer_cursor (consumer_name, last_sequence, version, updated_at)
             VALUES ('task-orchestrator-wakes', ?, 1, ?)
             ON CONFLICT(consumer_name) DO UPDATE SET
                 last_sequence = excluded.last_sequence,
                 version = event_consumer_cursor.version + 1,
                 updated_at = excluded.updated_at",
        )
        .bind(head)
        .bind(now)
        .execute(database.pool())
        .await
        .expect("consumer cursor advances to its high-water mark");
    }

    #[tokio::test]
    async fn pr6_durable_scan_leaves_human_orchestrator_pending_without_harness() {
        use db::{
            CreateProject, CreateRoleMembership, CreateTask, CreateTaskRole, DomainEventRepo,
            ProjectRepo, RoleMembershipRepo, TaskRepo, TaskRoleRepo, User, UserRepo,
        };

        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("schema migrates");
        let database = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(8));
        let now = now_rfc3339();
        let human_id = new_uuid_v4();
        UserRepo::create_user(
            &*database,
            &User {
                id: human_id.clone(),
                email: format!("{human_id}@example.test"),
                password_hash: "test-only".to_owned(),
                display_name: Some("Human orchestrator".to_owned()),
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("real Human Actor exists");
        let project_id = new_uuid_v4();
        ProjectRepo::create(
            &*database,
            CreateProject {
                id: project_id.clone(),
                name: "PR6 Human wake".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(human_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project creates");
        let task_id = new_uuid_v4();
        TaskRepo::create(
            &*database,
            CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Handle a durable orchestration wake".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                plan: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task creates");
        let task_role_id = new_uuid_v4();
        TaskRoleRepo::create(
            &*database,
            CreateTaskRole {
                id: task_role_id.clone(),
                task_id: task_id.clone(),
                role: "orchestrator".to_owned(),
                coordination_mode: Some(CoordinationMode::Collaborative),
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("canonical TaskRole creates");
        RoleMembershipRepo::add(
            &*database,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id,
                actor_kind: ActorKind::Human,
                actor_id: human_id,
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human membership activates");
        let existing_sequence: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM domain_event")
                .fetch_one(database.pool())
                .await
                .expect("current durable event sequence is queryable");
        sqlx::query(
            "INSERT INTO event_consumer_cursor (consumer_name, last_sequence, version, updated_at)
             VALUES ('task-orchestrator-wakes', ?, 1, ?)
             ON CONFLICT(consumer_name) DO UPDATE SET
                 last_sequence = excluded.last_sequence,
                 version = event_consumer_cursor.version + 1,
                 updated_at = excluded.updated_at",
        )
        .bind(existing_sequence)
        .bind(now_rfc3339())
        .execute(database.pool())
        .await
        .expect("test starts its consumer at the durable backlog boundary");
        let event = DomainEventRepo::append_event(
            &*database,
            db::CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.transitioned".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now,
            },
        )
        .await
        .expect("durable event appends without a bus publication");
        let task_service = Arc::new(TaskService::new(
            Arc::clone(&database),
            Arc::clone(&event_bus),
        ));
        let runtime = OrchestratorRuntime::new(Arc::clone(&database), event_bus, task_service);

        let outcome = runtime
            .run_once(10)
            .await
            .expect("durable event is processed");
        assert_eq!(outcome.processed_events, 1);
        assert_eq!(outcome.admitted_wakes, 1);
        assert_eq!(outcome.dispatched_wakes, 1);
        let state: String = sqlx::query_scalar(
            "SELECT state FROM orchestrator_wake WHERE event_id = ? AND task_id = ?",
        )
        .bind(&event.id)
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("Human obligation is durable");
        assert_eq!(state, "awaiting_human");
        let executions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(database.pool())
                .await
                .expect("Execution count is queryable");
        let sessions: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM harness_session")
            .fetch_one(database.pool())
            .await
            .expect("HarnessSession count is queryable");
        assert_eq!(executions, 0);
        assert_eq!(sessions, 0);
    }

    #[tokio::test]
    async fn pr6_new_task_member_activation_is_durable_without_eventbus_authority() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("schema migrates");
        let database = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(8));
        let human_id = test_user(&database, "Bootstrap Human").await;
        let project_id = test_project(&database, &human_id).await;
        let task_id = test_task(&database, &project_id, "New task with orchestrator").await;
        let task_created_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'task.created' AND entity_type = 'task' AND entity_id = ?",
        )
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("Task creation has a durable source event");
        assert_eq!(task_created_events, 1);
        let role_id = test_orchestrator_role(
            &database,
            &task_id,
            Some(CoordinationMode::Collaborative),
            "{}",
        )
        .await;
        let membership_id = test_membership(
            &database,
            &role_id,
            &human_id,
            RoleMembershipStatus::Suspended,
        )
        .await;
        set_orchestrator_cursor_to_head(&database).await;

        let task_service = Arc::new(TaskService::new(
            Arc::clone(&database),
            Arc::clone(&event_bus),
        ));
        let runtime = OrchestratorRuntime::new(
            Arc::clone(&database),
            Arc::clone(&event_bus),
            Arc::clone(&task_service),
        );

        let inactive = runtime
            .run_once(10)
            .await
            .expect("inactive state reconciles");
        assert_eq!(inactive.processed_events, 0);
        let wake_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM orchestrator_wake WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(database.pool())
                .await
                .expect("wake count loads");
        assert_eq!(wake_count, 0, "suspended membership is not eligible");

        task_service
            .update_task_role_member(&task_id, &membership_id, 1, RoleMembershipStatus::Active)
            .await
            .expect("membership activates through the TaskService writer");
        let membership_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'orchestrator.membership_changed' AND scope_id = ?",
        )
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("activation is recorded in the durable ledger");
        assert_eq!(membership_events, 1);
        let activated = runtime
            .run_once(10)
            .await
            .expect("durable membership event is processed without a bus hint");
        assert_eq!(activated.claimed_events, 1);
        assert_eq!(activated.admitted_wakes, 1);
        assert_eq!(activated.dispatched_wakes, 1);

        let states: Vec<String> = sqlx::query_scalar(
            "SELECT state FROM orchestrator_wake WHERE task_id = ? AND task_role_id = ?",
        )
        .bind(&task_id)
        .bind(&role_id)
        .fetch_all(database.pool())
        .await
        .expect("Human obligation is durable");
        assert_eq!(states, ["awaiting_human"]);
        let execution_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND role = 'orchestrator'",
        )
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("Execution count loads");
        assert_eq!(
            execution_count, 0,
            "Human wake creates no Harness Execution"
        );

        let new_task_id = test_task(
            &database,
            &project_id,
            "Task created with an eligible member",
        )
        .await;
        let new_role_id = test_orchestrator_role(
            &database,
            &new_task_id,
            Some(CoordinationMode::Collaborative),
            "{}",
        )
        .await;
        test_membership(
            &database,
            &new_role_id,
            &human_id,
            RoleMembershipStatus::Active,
        )
        .await;
        let new_task = runtime
            .run_once(10)
            .await
            .expect("new Task activation is found by the durable scan");
        assert_eq!(new_task.processed_events, 2);
        assert_eq!(new_task.admitted_wakes, 1);
        assert_eq!(new_task.dispatched_wakes, 1);
        let new_task_wake: String = sqlx::query_scalar(
            "SELECT state FROM orchestrator_wake WHERE task_id = ? AND task_role_id = ?",
        )
        .bind(&new_task_id)
        .bind(&new_role_id)
        .fetch_one(database.pool())
        .await
        .expect("new Task has one durable Human obligation");
        assert_eq!(new_task_wake, "awaiting_human");
    }

    #[tokio::test]
    async fn pr6_repeated_task_block_metadata_changes_are_durable() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("schema migrates");
        let database = Arc::new(db::SqliteDb::new(pool));
        let human_id = test_user(&database, "Blocked Task Human").await;
        let project_id = test_project(&database, &human_id).await;
        let task_id = test_task(&database, &project_id, "Repeated block reason").await;

        for reason in ["first", "second", "second"] {
            sqlx::query(
                "UPDATE task SET blocked_json = ?, version = version + 1,
                                  updated_at = ? WHERE id = ?",
            )
            .bind(json!({ "reason": reason }).to_string())
            .bind(now_rfc3339())
            .bind(&task_id)
            .execute(database.pool())
            .await
            .expect("Task block metadata updates");
        }

        let event_types: Vec<String> = sqlx::query_scalar(
            "SELECT event_type FROM domain_event
             WHERE event_type IN ('task.blocked', 'task.unblocked', 'task.failed')
               AND entity_id = ? ORDER BY sequence",
        )
        .bind(&task_id)
        .fetch_all(database.pool())
        .await
        .expect("Task state signals are durable");
        assert_eq!(event_types, ["task.blocked", "task.blocked"]);
    }

    #[tokio::test]
    async fn pr6_task_role_policy_change_after_admission_fails_the_old_wake() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("schema migrates");
        let database = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(8));
        let human_id = test_user(&database, "Policy snapshot Human").await;
        let project_id = test_project(&database, &human_id).await;
        let task_id = test_task(&database, &project_id, "Policy snapshot task").await;
        let role_id = test_orchestrator_role(
            &database,
            &task_id,
            Some(CoordinationMode::Collaborative),
            "{}",
        )
        .await;
        test_membership(&database, &role_id, &human_id, RoleMembershipStatus::Active).await;
        let unchanged_role = TaskRoleRepo::update(
            &*database,
            db::UpdateTaskRole {
                id: role_id.clone(),
                expected_version: 1,
                coordination_mode: Some(Some(CoordinationMode::Collaborative)),
                policy_json: Some("{}".to_owned()),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("a semantically unchanged policy update is accepted");
        assert_eq!(unchanged_role.version, 1);
        let unchanged_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'orchestrator.task_role_changed' AND entity_id = ?",
        )
        .bind(&role_id)
        .fetch_one(database.pool())
        .await
        .expect("no-op policy event count loads");
        assert_eq!(unchanged_events, 0, "no-op updates do not invalidate wakes");
        let event = DomainEventRepo::append_event(
            &*database,
            db::CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.transitioned".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("durable source event appends");
        let task_service = Arc::new(TaskService::new(
            Arc::clone(&database),
            Arc::clone(&event_bus),
        ));
        let runtime = OrchestratorRuntime::new(Arc::clone(&database), event_bus, task_service);
        assert_eq!(
            runtime
                .admit_signal(
                    &event,
                    WakeSignal {
                        work_unit_id: None,
                        target: WakeTarget::Task,
                    },
                )
                .await
                .expect("wake admission captures TaskRole policy"),
            1
        );

        TaskRoleRepo::update(
            &*database,
            db::UpdateTaskRole {
                id: role_id.clone(),
                expected_version: 1,
                coordination_mode: None,
                policy_json: Some(r#"{"automatic_orchestration":false}"#.to_owned()),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("TaskRole policy changes after admission");
        let policy_change_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'orchestrator.task_role_changed' AND entity_id = ?",
        )
        .bind(&role_id)
        .fetch_one(database.pool())
        .await
        .expect("policy change has a durable event");
        assert_eq!(policy_change_events, 1);

        assert!(matches!(
            runtime
                .dispatch_one()
                .await
                .expect("wake dispatch resolves"),
            DispatchOutcome::Retried
        ));
        let (state, last_error): (String, Option<String>) =
            sqlx::query_as("SELECT state, last_error FROM orchestrator_wake WHERE task_id = ?")
                .bind(&task_id)
                .fetch_one(database.pool())
                .await
                .expect("stale wake failure is durable");
        assert_eq!(state, "failed");
        assert!(last_error
            .as_deref()
            .is_some_and(|error| error.contains("TaskRole version or policy changed")));
        let executions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND role = 'orchestrator'",
        )
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("dispatch did not create an Execution");
        assert_eq!(executions, 0);
    }

    #[tokio::test]
    async fn pr6_reserved_action_losing_policy_race_fails_its_completed_wake() {
        use db::{
            AgentStatus, CreateAgent, CreateDomainEvent, CreateExecution, CreateRoleMembership,
            DomainEventRepo, ExecutionRepo, ExecutionStatus, OrchestratorWakeRepo,
            ReserveOrchestratorAction, RoleMembershipRepo, TaskRoleRepo, UpdateTaskRole,
        };

        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("schema migrates");
        let database = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(8));
        let human_id = test_user(&database, "Action policy race owner").await;
        let project_id = test_project(&database, &human_id).await;
        let task_id = test_task(&database, &project_id, "Action policy race").await;
        let now = now_rfc3339();
        let agent_id = new_uuid_v4();
        AgentRepo::create(
            &*database,
            CreateAgent {
                id: agent_id.clone(),
                name: "PR6 action race Agent".to_owned(),
                description: None,
                executor_type: "codex".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: None,
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Agent Actor creates");
        let role_id = test_orchestrator_role(
            &database,
            &task_id,
            Some(CoordinationMode::Collaborative),
            "{}",
        )
        .await;
        RoleMembershipRepo::add(
            &*database,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: role_id.clone(),
                actor_kind: ActorKind::Agent,
                actor_id: agent_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Agent is an active orchestrator member");
        let source_event = DomainEventRepo::append_event(
            &*database,
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.transitioned".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("durable source event appends");
        let task_service = Arc::new(TaskService::new(
            Arc::clone(&database),
            Arc::clone(&event_bus),
        ));
        let runtime =
            OrchestratorRuntime::new(Arc::clone(&database), Arc::clone(&event_bus), task_service);
        assert_eq!(
            runtime
                .admit_signal(
                    &source_event,
                    WakeSignal {
                        work_unit_id: None,
                        target: WakeTarget::Task,
                    },
                )
                .await
                .expect("wake admits under policy P1"),
            1
        );
        let wake_id: String = sqlx::query_scalar(
            "SELECT id FROM orchestrator_wake WHERE event_id = ? AND task_id = ?",
        )
        .bind(&source_event.id)
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("exact wake id loads");
        let lease_owner = "pr6-action-policy-race";
        let claim_now = now_rfc3339();
        OrchestratorWakeRepo::claim_orchestrator_wake(
            &*database,
            db::ClaimOrchestratorWake {
                lease_owner: lease_owner.to_owned(),
                now: claim_now,
                leased_until: "2026-10-01T00:00:00Z".to_owned(),
            },
        )
        .await
        .expect("wake claim succeeds")
        .expect("wake is leased");
        let execution_id = new_uuid_v4();
        let attempt = OrchestratorWakeRepo::reserve_orchestrator_wake_execution(
            &*database,
            db::ReserveOrchestratorWakeExecution {
                wake_id: wake_id.clone(),
                lease_owner: lease_owner.to_owned(),
                execution_id: execution_id.clone(),
                now: now.clone(),
            },
        )
        .await
        .expect("exact wake attempt reserves");
        let message_action = OrchestratorAction::Message {
            target: ActionTarget::Task,
            work_unit_id: None,
            body: "A stable action result".to_owned(),
        };
        let action_digest = hex::encode(sha2::Sha256::digest(
            serde_json::to_vec(&message_action).expect("message action serializes"),
        ));
        let action_response = json!({"actions": [message_action]}).to_string();
        let start_event_id = new_uuid_v4();
        ExecutionRepo::create_orchestrator_execution(
            &*database,
            CreateExecution {
                id: execution_id.clone(),
                task_id: task_id.clone(),
                agent_id: Some(agent_id.clone()),
                actor_ref: Some(ActorRef::Agent(agent_id.clone())),
                role: "orchestrator".to_owned(),
                purpose: Some(ExecutionPurpose::Orchestrate),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                harness_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(action_response),
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: Some("{}".to_owned()),
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            &wake_id,
            attempt.attempt_number,
            lease_owner,
            CreateDomainEvent {
                id: start_event_id.clone(),
                event_type: "execution.started".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: execution_id.clone(),
                actor_type: "agent".to_owned(),
                actor_id: Some(agent_id.clone()),
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: source_event.correlation_id.clone(),
                causation_id: Some(source_event.id.clone()),
                causation_depth: 1,
                dedupe_key: Some(format!("execution.started:{execution_id}")),
                payload_json: "{}".to_owned(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("exact orchestrator Execution starts");
        assert!(
            OrchestratorWakeRepo::transition_orchestrator_wake_execution(
                &*database,
                db::TransitionOrchestratorWakeExecution {
                    wake_id: wake_id.clone(),
                    attempt_number: attempt.attempt_number,
                    execution_id: execution_id.clone(),
                    lease_owner: None,
                    expected_state: Some("reserved".to_owned()),
                    state: "running".to_owned(),
                    last_error: None,
                    updated_at: now.clone(),
                },
            )
            .await
            .expect("attempt enters running state")
        );
        assert!(OrchestratorWakeRepo::transition_orchestrator_wake(
            &*database,
            db::TransitionOrchestratorWake {
                id: wake_id.clone(),
                expected_state: Some(OrchestratorWakeState::Leased),
                lease_owner: None,
                state: OrchestratorWakeState::Running,
                available_at: None,
                current_attempt: None,
                last_error: None,
                updated_at: now.clone(),
            },
        )
        .await
        .expect("wake enters terminal reconciliation state"));
        ExecutionRepo::update(
            &*database,
            db::UpdateExecution {
                id: execution_id.clone(),
                status: Some(ExecutionStatus::Completed),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now.clone(),
            },
        )
        .await
        .expect("orchestrator Execution completes");
        let action_result_id = new_uuid_v4();
        OrchestratorWakeRepo::reserve_orchestrator_action(
            &*database,
            ReserveOrchestratorAction {
                execution_id: execution_id.clone(),
                action_index: 0,
                action_type: "message".to_owned(),
                action_digest,
                result_id: action_result_id.clone(),
                now: now.clone(),
            },
        )
        .await
        .expect("typed action reserves with its stable result id under P1");
        TaskRoleRepo::update(
            &*database,
            UpdateTaskRole {
                id: role_id,
                expected_version: 1,
                coordination_mode: None,
                policy_json: Some(r#"{"allowed_actions":["proposal"]}"#.to_owned()),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("TaskRole changes from P1 to P2");

        let start_event = DomainEventRepo::get_event_by_dedupe(
            &*database,
            &format!("execution.started:{execution_id}"),
        )
        .await
        .expect("start event lookup succeeds")
        .expect("start event exists");
        let mut transaction = database
            .pool()
            .begin()
            .await
            .expect("write transaction begins");
        sqlx::query(
            "INSERT INTO message (
                id, task_id, sender_actor_kind, sender_actor_id, target_kind,
                target_actor_kind, target_actor_id, target_role_id, body, created_at
             ) VALUES (?, ?, 'agent', ?, 'task', NULL, NULL, NULL, 'stale output', ?)",
        )
        .bind(&action_result_id)
        .bind(&task_id)
        .bind(&agent_id)
        .bind(&now)
        .execute(&mut *transaction)
        .await
        .expect("effect row inserts before its guarded event");
        let stale_event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "message.created".to_owned(),
            entity_type: "message".to_owned(),
            entity_id: action_result_id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: Some(agent_id.clone()),
            scope_type: "task".to_owned(),
            scope_id: task_id.clone(),
            correlation_id: start_event.correlation_id,
            causation_id: Some(start_event.id),
            causation_depth: 2,
            dedupe_key: None,
            payload_json: "{}".to_owned(),
            created_at: now.clone(),
        };
        let guard_error =
            DomainEventRepo::append_event_in_tx(&*database, &mut transaction, &stale_event)
                .await
                .expect_err("the policy guard rejects the output in its effect transaction");
        transaction
            .rollback()
            .await
            .expect("rejected effect transaction rolls back");
        assert!(matches!(guard_error, db::DbError::StaleOrchestratorAction));
        let effect_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM message WHERE id = ?")
            .bind(&action_result_id)
            .fetch_one(database.pool())
            .await
            .expect("rolled-back effect count loads");
        let event_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE event_type = 'message.created' AND entity_id = ?",
        )
        .bind(&action_result_id)
        .fetch_one(database.pool())
        .await
        .expect("rolled-back event count loads");
        assert_eq!(effect_count, 0);
        assert_eq!(event_count, 0);

        let terminal_event = DomainEventRepo::append_event(
            &*database,
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "execution.completed".to_owned(),
                entity_type: "execution".to_owned(),
                entity_id: execution_id.clone(),
                actor_type: "agent".to_owned(),
                actor_id: Some(agent_id),
                scope_type: "task".to_owned(),
                scope_id: task_id,
                correlation_id: source_event.correlation_id,
                causation_id: Some(start_event_id),
                causation_depth: 2,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now_rfc3339(),
            },
        )
        .await
        .expect("terminal event appends");
        assert!(runtime
            .reconcile_orchestrator_terminal(&terminal_event)
            .await
            .expect("stale completed wake reconciles deterministically"));
        let wake = OrchestratorWakeRepo::get_orchestrator_wake(&*database, &wake_id)
            .await
            .expect("wake lookup succeeds")
            .expect("wake remains durable");
        assert_eq!(wake.state, OrchestratorWakeState::Failed);
        assert!(wake.last_error.as_deref().is_some_and(
            |error| error.contains("TaskRole version, coordination, or policy changed")
        ));
        let action_state: String =
            sqlx::query_scalar("SELECT state FROM orchestrator_action WHERE result_id = ?")
                .bind(&action_result_id)
                .fetch_one(database.pool())
                .await
                .expect("stale action reservation remains auditable");
        assert_eq!(action_state, "reserved");
    }

    #[tokio::test]
    async fn pr6_v095_bootstrap_reconciles_head_cursor_once_with_mode_exact_targets() {
        let source_migrations =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../db/migrations");
        let pre_v095 = tempfile::tempdir().expect("pre-V095 migration directory creates");
        for entry in std::fs::read_dir(&source_migrations).expect("migration directory reads") {
            let entry = entry.expect("migration entry reads");
            let filename = entry.file_name();
            let filename = filename.to_string_lossy();
            if filename.starts_with("V095__") || filename.starts_with("V096__") {
                continue;
            }
            std::fs::copy(entry.path(), pre_v095.path().join(filename.as_ref()))
                .expect("pre-V095 migration copies");
        }

        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations_from(&pool, pre_v095.path())
            .await
            .expect("schema stops at V094");
        let database = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(8));
        let human_a = test_user(&database, "Bootstrap A").await;
        let human_b = test_user(&database, "Bootstrap B").await;
        let project_id = test_project(&database, &human_a).await;

        let collaborative_task =
            test_task(&database, &project_id, "Collaborative pre-cutover").await;
        let collaborative_role = test_orchestrator_role(
            &database,
            &collaborative_task,
            Some(CoordinationMode::Collaborative),
            "{}",
        )
        .await;
        test_membership(
            &database,
            &collaborative_role,
            &human_a,
            RoleMembershipStatus::Active,
        )
        .await;
        test_membership(
            &database,
            &collaborative_role,
            &human_b,
            RoleMembershipStatus::Active,
        )
        .await;

        let independent_task = test_task(&database, &project_id, "Independent pre-cutover").await;
        let independent_role = test_orchestrator_role(
            &database,
            &independent_task,
            Some(CoordinationMode::Independent),
            "{}",
        )
        .await;
        test_membership(
            &database,
            &independent_role,
            &human_a,
            RoleMembershipStatus::Active,
        )
        .await;
        test_membership(
            &database,
            &independent_role,
            &human_b,
            RoleMembershipStatus::Active,
        )
        .await;

        let independent_unique_task =
            test_task(&database, &project_id, "Independent unique pre-cutover").await;
        let independent_unique_role = test_orchestrator_role(
            &database,
            &independent_unique_task,
            Some(CoordinationMode::Independent),
            "{}",
        )
        .await;
        test_membership(
            &database,
            &independent_unique_role,
            &human_a,
            RoleMembershipStatus::Active,
        )
        .await;

        let partitioned_task = test_task(&database, &project_id, "Partitioned pre-cutover").await;
        let partitioned_role = test_orchestrator_role(
            &database,
            &partitioned_task,
            Some(CoordinationMode::Partitioned),
            "{}",
        )
        .await;
        test_membership(
            &database,
            &partitioned_role,
            &human_a,
            RoleMembershipStatus::Active,
        )
        .await;
        test_membership(
            &database,
            &partitioned_role,
            &human_b,
            RoleMembershipStatus::Active,
        )
        .await;
        let work_unit_id = new_uuid_v4();
        let work_unit_created_at = now_rfc3339();
        WorkUnitRepo::create(
            &*database,
            db::CreateWorkUnit {
                id: work_unit_id.clone(),
                task_id: partitioned_task.clone(),
                parent_work_unit_id: None,
                title: "Exact orchestrator assignment".to_owned(),
                scope: "One explicit orchestrator allocation".to_owned(),
                role: "orchestrator".to_owned(),
                assigned_actor: Some(ActorRef::Human(human_a.clone())),
                requires_integration: false,
                provenance: None,
                created_by: ActorRef::Human(human_a.clone()),
                created_at: work_unit_created_at.clone(),
            },
            db::CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "work_unit.created".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: work_unit_id,
                actor_type: "human".to_owned(),
                actor_id: Some(human_a.clone()),
                scope_type: "task".to_owned(),
                scope_id: partitioned_task,
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: r#"{"role":"orchestrator"}"#.to_owned(),
                created_at: work_unit_created_at,
            },
        )
        .await
        .expect("partitioned exact WorkUnit allocation creates");

        set_orchestrator_cursor_to_head(&database).await;
        let before_v095_head: i64 = sqlx::query_scalar(
            "SELECT last_sequence FROM event_consumer_cursor WHERE consumer_name = 'task-orchestrator-wakes'",
        )
        .fetch_one(database.pool())
        .await
        .expect("consumer cursor is at the pre-upgrade head");

        db::run_migrations_from(&database.pool(), &source_migrations)
            .await
            .expect("V095 applies after the consumer cursor is already at head");
        let seeded_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event WHERE event_type = 'orchestrator.bootstrap_reconciled'",
        )
        .fetch_one(database.pool())
        .await
        .expect("bootstrap events are durable");
        assert_eq!(
            seeded_events, 4,
            "collaborative fans out; independent requires a unique member; partitioned requires assignment"
        );
        let ambiguous_independent_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'orchestrator.bootstrap_reconciled' AND entity_id = ?",
        )
        .bind(&independent_role)
        .fetch_one(database.pool())
        .await
        .expect("ambiguous independent role is not bootstrapped");
        assert_eq!(ambiguous_independent_events, 0);
        let cursor_after_upgrade: i64 = sqlx::query_scalar(
            "SELECT last_sequence FROM event_consumer_cursor WHERE consumer_name = 'task-orchestrator-wakes'",
        )
        .fetch_one(database.pool())
        .await
        .expect("upgrade preserves the old cursor");
        assert_eq!(cursor_after_upgrade, before_v095_head);

        let task_service = Arc::new(TaskService::new(
            Arc::clone(&database),
            Arc::clone(&event_bus),
        ));
        let runtime =
            OrchestratorRuntime::new(Arc::clone(&database), Arc::clone(&event_bus), task_service);
        let reconciled = runtime
            .run_once(20)
            .await
            .expect("bootstrap obligations run through the durable wake consumer");
        assert_eq!(reconciled.processed_events, 4);
        assert_eq!(reconciled.admitted_wakes, 4);
        assert_eq!(reconciled.dispatched_wakes, 4);

        let mode_counts: Vec<(String, i64)> = sqlx::query_as(
            "SELECT tr.coordination_mode, COUNT(*)
             FROM orchestrator_wake wake
             JOIN task_role tr ON tr.id = wake.task_role_id
             GROUP BY tr.coordination_mode ORDER BY tr.coordination_mode",
        )
        .fetch_all(database.pool())
        .await
        .expect("bootstrap targeting follows each coordination mode");
        assert_eq!(
            mode_counts,
            vec![
                ("collaborative".to_owned(), 2),
                ("independent".to_owned(), 1),
                ("partitioned".to_owned(), 1),
            ]
        );
        let pending_human: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM orchestrator_wake WHERE state = 'awaiting_human'",
        )
        .fetch_one(database.pool())
        .await
        .expect("Human bootstrap work stays pending");
        assert_eq!(pending_human, 4);
        let executions: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM execution WHERE role = 'orchestrator'")
                .fetch_one(database.pool())
                .await
                .expect("Human bootstrap does not create Executions");
        assert_eq!(executions, 0);

        db::run_migrations_from(&database.pool(), &source_migrations)
            .await
            .expect("migration discovery remains idempotent");
        let repeated = runtime
            .run_once(20)
            .await
            .expect("repeated reconciliation has no new bootstrap event");
        assert_eq!(repeated.claimed_events, 0);
        let wake_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM orchestrator_wake")
            .fetch_one(database.pool())
            .await
            .expect("wake count remains stable");
        assert_eq!(wake_count, 4);
    }

    #[derive(Clone)]
    struct Pr6RecordingExecutor {
        response: String,
        observed:
            Arc<std::sync::Mutex<Vec<(api_types::HarnessInvocation, Option<String>, String)>>>,
    }

    #[async_trait::async_trait]
    impl executors::TaskExecutor for Pr6RecordingExecutor {
        async fn execute(
            &self,
            context: executors::ExecutionContext,
        ) -> std::result::Result<executors::ExecutionResult, executors::ExecutorError> {
            self.observed.lock().expect("observation mutex").push((
                context.invocation,
                context
                    .agent_config
                    .get("config")
                    .and_then(|config| config.get("sandbox"))
                    .and_then(Value::as_str)
                    .map(str::to_owned),
                context.role,
            ));
            Ok(executors::ExecutionResult {
                status: executors::ExecutionOutcome::Completed,
                summary: Some(self.response.clone()),
                ..Default::default()
            })
        }

        async fn cancel(
            &self,
            _execution_id: &str,
        ) -> std::result::Result<(), executors::ExecutorError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn pr6_agent_capacity_retries_then_starts_fresh_execution_and_replays_typed_actions() {
        use db::{
            ActorKind, AgentRepo, AgentStatus, CoordinationMode, CreateAgent, CreateDomainEvent,
            CreateExecution, CreateProject, CreateRoleMembership, CreateTask, CreateTaskRole,
            DaemonRepo, DaemonStatus, DomainEventRepo, ExecutionRepo, ExecutionStatus,
            HarnessSessionRepo, ProjectRepo, RoleMembershipRepo, RoleMembershipStatus, TaskRepo,
            TaskRoleRepo, UpdateDaemonReport, UpsertDaemon, WorkUnitRepo,
        };

        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("schema migrates");
        let database = Arc::new(db::SqliteDb::new(pool));
        let event_bus = Arc::new(EventBus::new(16));
        let now = now_rfc3339();

        let daemon_id = new_uuid_v4();
        DaemonRepo::upsert_by_machine_id(
            &*database,
            UpsertDaemon {
                id: daemon_id.clone(),
                machine_id: format!("pr6-{daemon_id}"),
                hostname: "pr6-test".to_owned(),
                os: "linux".to_owned(),
                arch: "x86_64".to_owned(),
                agent_version: None,
                labels_json: "{}".to_owned(),
                status: DaemonStatus::Online,
                registration_token_hash: None,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("online daemon creates");
        DaemonRepo::update_report(
            &*database,
            UpdateDaemonReport {
                id: daemon_id.clone(),
                detected_clis_json: r#"[{"kind":"codex","availability":"authenticated"}]"#
                    .to_owned(),
                labels_json: None,
                status: DaemonStatus::Online,
                last_report_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Codex capability is advertised");
        let agent_id = new_uuid_v4();
        AgentRepo::create(
            &*database,
            CreateAgent {
                id: agent_id.clone(),
                name: "PR6 orchestrator".to_owned(),
                description: None,
                executor_type: "codex".to_owned(),
                model: None,
                reasoning_effort: None,
                permission_policy: None,
                prompt_template: None,
                capabilities_json: "[]".to_owned(),
                config_json: "{}".to_owned(),
                credential_ref: None,
                daemon_id: Some(daemon_id),
                max_concurrent_tasks: 1,
                heartbeat_interval_seconds: 30,
                max_missed_heartbeats: 3,
                status: AgentStatus::Idle,
                last_heartbeat_at: None,
                is_default: false,
                paused: false,
                owner_id: None,
                visibility: "global".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("CLI Agent creates");

        let project_id = new_uuid_v4();
        ProjectRepo::create(
            &*database,
            CreateProject {
                id: project_id.clone(),
                name: "PR6 Agent wake".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("project creates");
        let task_id = new_uuid_v4();
        TaskRepo::create(
            &*database,
            CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Create bounded work and request merge approval".to_owned(),
                description: Some("Only a small task-scoped action set is authorized.".to_owned()),
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                plan: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("task creates");
        let orchestrator_role_id = new_uuid_v4();
        TaskRoleRepo::create(
            &*database,
            db::CreateTaskRole {
                id: orchestrator_role_id.clone(),
                task_id: task_id.clone(),
                role: "orchestrator".to_owned(),
                coordination_mode: Some(CoordinationMode::Collaborative),
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("orchestrator TaskRole creates");
        RoleMembershipRepo::add(
            &*database,
            CreateRoleMembership {
                id: new_uuid_v4(),
                task_role_id: orchestrator_role_id.clone(),
                actor_kind: ActorKind::Agent,
                actor_id: agent_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Agent holds active orchestrator membership");
        TaskRoleRepo::create(
            &*database,
            CreateTaskRole {
                id: new_uuid_v4(),
                task_id: task_id.clone(),
                role: "implementer".to_owned(),
                coordination_mode: Some(CoordinationMode::Independent),
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("bounded WorkUnit target role exists");
        let existing_sequence: i64 =
            sqlx::query_scalar("SELECT COALESCE(MAX(sequence), 0) FROM domain_event")
                .fetch_one(database.pool())
                .await
                .expect("current durable event sequence is queryable");
        sqlx::query(
            "INSERT INTO event_consumer_cursor (consumer_name, last_sequence, version, updated_at)
             VALUES ('task-orchestrator-wakes', ?, 1, ?)
             ON CONFLICT(consumer_name) DO UPDATE SET
                 last_sequence = excluded.last_sequence,
                 version = event_consumer_cursor.version + 1,
                 updated_at = excluded.updated_at",
        )
        .bind(existing_sequence)
        .bind(now_rfc3339())
        .execute(database.pool())
        .await
        .expect("test starts its consumer at the durable backlog boundary");

        let worker_execution_id = new_uuid_v4();
        ExecutionRepo::create(
            &*database,
            CreateExecution {
                id: worker_execution_id.clone(),
                task_id: task_id.clone(),
                agent_id: Some(agent_id.clone()),
                actor_ref: Some(ActorRef::Agent(agent_id.clone())),
                role: "implementer".to_owned(),
                purpose: Some(ExecutionPurpose::Implement),
                status: ExecutionStatus::Running,
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                parent_execution_id: None,
                agent_session_id: None,
                harness_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: None,
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                workspace_id: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("busy Worker execution creates");

        let cause = DomainEventRepo::append_event(
            &*database,
            CreateDomainEvent {
                id: new_uuid_v4(),
                event_type: "task.transitioned".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: new_uuid_v4(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json: "{}".to_owned(),
                created_at: now.clone(),
            },
        )
        .await
        .expect("durable cause appends without publishing EventBus hint");
        let action_response = serde_json::json!({
            "actions": [
                {
                    "type": "create_work_unit",
                    "title": "Bounded follow-up",
                    "scope": "Implement one isolated behavior",
                    "role": "implementer",
                    "parent_work_unit_id": null,
                    "assigned_actor": null,
                    "requires_integration": false
                },
                {
                    "type": "proposal",
                    "target": {"kind": "task", "id": task_id},
                    "action": "merge",
                    "reason": "The integration workspace is ready for explicit human review.",
                    "target_version": 1,
                    "target_digest": null
                }
            ]
        })
        .to_string();
        let observed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let workspace_root = tempfile::tempdir().expect("workspace root creates");
        let task_service = Arc::new(
            TaskService::new(Arc::clone(&database), Arc::clone(&event_bus))
                .with_task_executor(Arc::new(Pr6RecordingExecutor {
                    response: action_response,
                    observed: Arc::clone(&observed),
                }))
                .with_adapter_registry(Arc::new(cli_adapters::default_registry()))
                .with_workspace_root(workspace_root.path().to_path_buf()),
        );
        let runtime =
            OrchestratorRuntime::new(Arc::clone(&database), Arc::clone(&event_bus), task_service);

        let deferred = runtime
            .run_once(10)
            .await
            .expect("busy wake is consumed durably");
        assert_eq!(deferred.admitted_wakes, 1);
        assert_eq!(deferred.retried_wakes, 1);
        let wake_id: String = sqlx::query_scalar(
            "SELECT id FROM orchestrator_wake WHERE event_id = ? AND task_id = ?",
        )
        .bind(&cause.id)
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("wake obligation exists");
        let pending_state: String =
            sqlx::query_scalar("SELECT state FROM orchestrator_wake WHERE id = ?")
                .bind(&wake_id)
                .fetch_one(database.pool())
                .await
                .expect("retry state is stored");
        assert_eq!(pending_state, "pending");
        let attempts_before_capacity_clears: i64 =
            sqlx::query_scalar("SELECT attempt_count FROM orchestrator_wake WHERE id = ?")
                .bind(&wake_id)
                .fetch_one(database.pool())
                .await
                .expect("attempt count is stored");
        assert_eq!(attempts_before_capacity_clears, 0);

        ExecutionRepo::update(
            &*database,
            db::UpdateExecution {
                id: worker_execution_id,
                status: Some(ExecutionStatus::Completed),
                stop_reason: None,
                stopped_by: None,
                resume_policy: None,
                stopped_at: None,
                agent_session_id: None,
                agent_message_id: None,
                last_activity_at: None,
                summary: Some(Some("worker done".to_owned())),
                logs_path: None,
                before_sha: None,
                after_sha: None,
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("capacity frees");
        sqlx::query("UPDATE orchestrator_wake SET available_at = ? WHERE id = ?")
            .bind(now_rfc3339())
            .bind(&wake_id)
            .execute(database.pool())
            .await
            .expect("retry becomes due after simulated backoff");

        let dispatched = runtime.run_once(10).await.expect("Agent wake starts");
        let wake_diagnostic = sqlx::query_as::<_, (String, Option<String>)>(
            "SELECT state, last_error FROM orchestrator_wake WHERE id = ?",
        )
        .bind(&wake_id)
        .fetch_one(database.pool())
        .await
        .expect("wake state and failure reason are inspectable");
        assert_eq!(
            dispatched.dispatched_wakes, 1,
            "run outcome {dispatched:?}; wake state/error {wake_diagnostic:?}"
        );
        let mut orchestrator_execution = None;
        for _ in 0..100 {
            let attempt_number = OrchestratorWakeRepo::get_orchestrator_wake(&*database, &wake_id)
                .await
                .expect("wake loads")
                .expect("wake remains durable")
                .current_attempt;
            let execution_id = if let Some(attempt_number) = attempt_number {
                sqlx::query_scalar::<_, String>(
                    "SELECT execution_id FROM orchestrator_wake_execution WHERE wake_id = ? AND attempt_number = ?",
                )
                .bind(&wake_id)
                .bind(attempt_number)
                .fetch_optional(database.pool())
                .await
                .expect("Execution attempt lookup succeeds")
            } else {
                None
            };
            if let Some(execution_id) = execution_id {
                let execution = ExecutionRepo::get_by_id(&*database, &execution_id)
                    .await
                    .expect("Execution lookup succeeds");
                if execution
                    .as_ref()
                    .is_some_and(|row| row.status == ExecutionStatus::Completed)
                {
                    orchestrator_execution = execution;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let execution = orchestrator_execution.expect("fake Harness finishes the new Execution");
        assert_eq!(execution.role, "orchestrator");
        assert_eq!(execution.purpose, Some(ExecutionPurpose::Orchestrate));
        assert_eq!(
            execution.actor_ref(),
            Some(ActorRef::Agent(agent_id.clone()))
        );
        assert_eq!(execution.workspace_id, None);
        let session_id = execution
            .harness_session_id
            .as_deref()
            .expect("new Execution carries its exact explicit HarnessSession");
        let session = HarnessSessionRepo::get_by_id(&*database, session_id)
            .await
            .expect("HarnessSession lookup succeeds")
            .expect("Execution's exact HarnessSession exists");
        assert_eq!(session.agent_id, agent_id);
        assert_eq!(session.harness_kind, "codex");
        assert_eq!(session.workspace_id, None);
        let observations = observed.lock().expect("executor observations");
        assert_eq!(observations.len(), 1);
        assert!(matches!(
            &observations[0].0,
            api_types::HarnessInvocation::Start
        ));
        assert_eq!(observations[0].1.as_deref(), Some("read-only"));
        assert_eq!(observations[0].2, "orchestrator");
        drop(observations);

        let reconciled = runtime
            .run_once(10)
            .await
            .expect("terminal event applies actions");
        assert!(reconciled.processed_events >= 1);
        let wake = OrchestratorWakeRepo::get_orchestrator_wake(&*database, &wake_id)
            .await
            .expect("wake loads")
            .expect("wake persists");
        assert_eq!(wake.state, OrchestratorWakeState::Completed);
        let work_units = WorkUnitRepo::list_by_task(&*database, &task_id)
            .await
            .expect("bounded WorkUnit exists");
        assert_eq!(work_units.len(), 1);
        assert_eq!(work_units[0].title, "Bounded follow-up");
        let proposals = CollaborationRepo::list_proposals(&*database, &task_id, recent_page(10))
            .await
            .expect("protected action is only a proposal");
        assert_eq!(proposals.items.len(), 1);
        assert_eq!(proposals.items[0].action, "merge");
        let task = TaskRepo::get_by_id(&*database, &task_id, false)
            .await
            .expect("task loads")
            .expect("task exists");
        assert_eq!(task.status, "todo");
        let total_executions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM execution WHERE task_id = ? AND role = 'orchestrator' AND purpose = 'orchestrate'",
        )
        .bind(&task_id)
        .fetch_one(database.pool())
        .await
        .expect("Execution count is queryable");
        assert_eq!(total_executions, 1);

        let work_unit_event = sqlx::query(
            "SELECT id, correlation_id, causation_id, causation_depth
             FROM domain_event WHERE entity_type = 'work_unit' AND entity_id = ?",
        )
        .bind(&work_units[0].id)
        .fetch_one(database.pool())
        .await
        .expect("WorkUnit mutation event is durable");
        let start_event = DomainEventRepo::get_event_by_dedupe(
            &*database,
            &format!("execution.started:{}", execution.id),
        )
        .await
        .expect("Execution start event lookup succeeds")
        .expect("Execution start event exists");
        assert_eq!(
            work_unit_event
                .try_get::<Option<String>, _>("causation_id")
                .expect("cause reads"),
            Some(start_event.id.clone())
        );
        assert_eq!(
            work_unit_event
                .try_get::<String, _>("correlation_id")
                .expect("correlation reads"),
            start_event.correlation_id
        );
        assert_eq!(
            work_unit_event
                .try_get::<i64, _>("causation_depth")
                .expect("depth reads"),
            start_event.causation_depth + 1
        );

        runtime
            .apply_completed_actions(&execution, &wake)
            .await
            .expect("replayed action output is idempotent");
        let work_units_after_replay = WorkUnitRepo::list_by_task(&*database, &task_id)
            .await
            .expect("WorkUnit replay is queryable");
        let proposals_after_replay =
            CollaborationRepo::list_proposals(&*database, &task_id, recent_page(10))
                .await
                .expect("Proposal replay is queryable");
        assert_eq!(work_units_after_replay.len(), 1);
        assert_eq!(proposals_after_replay.items.len(), 1);

        TaskRoleRepo::update(
            &*database,
            db::UpdateTaskRole {
                id: orchestrator_role_id,
                expected_version: 1,
                coordination_mode: None,
                policy_json: Some(r#"{"allowed_actions":["message"]}"#.to_owned()),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("current action policy changes after the first replay");
        assert!(
            runtime
                .apply_completed_actions(&execution, &wake)
                .await
                .is_err(),
            "completed action replay cannot use a changed policy snapshot"
        );
        let work_units_after_policy_change = WorkUnitRepo::list_by_task(&*database, &task_id)
            .await
            .expect("WorkUnit state remains queryable");
        let proposals_after_policy_change =
            CollaborationRepo::list_proposals(&*database, &task_id, recent_page(10))
                .await
                .expect("Proposal state remains queryable");
        assert_eq!(work_units_after_policy_change.len(), 1);
        assert_eq!(proposals_after_policy_change.items.len(), 1);
    }

    fn member(kind: ActorKind, id: &str) -> RoleMembership {
        RoleMembership {
            id: format!("membership-{id}"),
            task_role_id: "role-orchestrator".to_owned(),
            actor_kind: kind,
            actor_id: id.to_owned(),
            status: RoleMembershipStatus::Active,
            version: 1,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
            ended_at: None,
        }
    }

    #[test]
    fn pr6_collaborative_fans_out_generic_task_work_without_sql_order_selection() {
        let members = vec![member(ActorKind::Agent, "b"), member(ActorKind::Agent, "a")];
        let targets = select_orchestrator_targets(
            Some(CoordinationMode::Collaborative),
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Task,
        );
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|member| member.actor_id == "a"));
        assert!(targets.iter().any(|member| member.actor_id == "b"));
    }

    #[test]
    fn pr6_partitioned_requires_the_exact_orchestrator_work_unit_assignment() {
        let members = vec![member(ActorKind::Agent, "a"), member(ActorKind::Agent, "b")];
        let unit = WorkUnit {
            id: "wu-1".to_owned(),
            task_id: "task-1".to_owned(),
            parent_work_unit_id: None,
            title: "bounded".to_owned(),
            scope: "one scope".to_owned(),
            status: WorkUnitStatus::Open,
            role: "orchestrator".to_owned(),
            assigned_actor: Some(ActorRef::Agent("b".to_owned())),
            requires_integration: false,
            provenance: None,
            created_by: ActorRef::Agent("a".to_owned()),
            version: 1,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        let targets = select_orchestrator_targets(
            Some(CoordinationMode::Partitioned),
            "role-orchestrator",
            &members,
            Some(&unit),
            &WakeTarget::Task,
        );
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].actor_id, "b");
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Partitioned),
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Task,
        )
        .is_empty());
    }

    #[test]
    fn pr6_independent_only_wakes_an_exact_or_uniquely_resolved_target() {
        let members = vec![member(ActorKind::Agent, "a"), member(ActorKind::Agent, "b")];
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Independent),
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Task,
        )
        .is_empty());
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Independent),
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Role("role-orchestrator".to_owned()),
        )
        .is_empty());
        let exact = select_orchestrator_targets(
            Some(CoordinationMode::Independent),
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Actor(ActorRef::Agent("b".to_owned())),
        );
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].actor_id, "b");
    }

    #[test]
    fn pr6_missing_mode_fails_closed_for_ambiguous_role_and_generic_wakes() {
        let members = vec![member(ActorKind::Agent, "a"), member(ActorKind::Agent, "b")];
        assert!(select_orchestrator_targets(
            None,
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Task
        )
        .is_empty());
        assert!(select_orchestrator_targets(
            None,
            "role-orchestrator",
            &members,
            None,
            &WakeTarget::Role("role-orchestrator".to_owned()),
        )
        .is_empty());
        assert_eq!(
            select_orchestrator_targets(
                None,
                "role-orchestrator",
                &members,
                None,
                &WakeTarget::Actor(ActorRef::Agent("a".to_owned())),
            )
            .len(),
            1
        );
    }

    #[test]
    fn pr6_role_addressed_collaboration_respects_each_supported_mode() {
        let members = vec![member(ActorKind::Agent, "a"), member(ActorKind::Agent, "b")];
        let role = WakeTarget::Role("role-orchestrator".to_owned());
        assert_eq!(
            select_orchestrator_targets(
                Some(CoordinationMode::Collaborative),
                "role-orchestrator",
                &members,
                None,
                &role,
            )
            .len(),
            2
        );
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Partitioned),
            "role-orchestrator",
            &members,
            None,
            &role,
        )
        .is_empty());
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Independent),
            "role-orchestrator",
            &members,
            None,
            &role,
        )
        .is_empty());
    }

    #[test]
    fn pr6_suspended_and_ended_memberships_are_never_wake_targets() {
        let mut suspended = member(ActorKind::Agent, "a");
        suspended.status = RoleMembershipStatus::Suspended;
        let mut ended = member(ActorKind::Agent, "b");
        ended.status = RoleMembershipStatus::Ended;
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Collaborative),
            "role-orchestrator",
            &[suspended.clone(), ended.clone()],
            None,
            &WakeTarget::Task,
        )
        .is_empty());
        assert!(select_orchestrator_targets(
            Some(CoordinationMode::Collaborative),
            "role-orchestrator",
            &[suspended.clone(), ended],
            None,
            &WakeTarget::Actor(ActorRef::Agent("a".to_owned())),
        )
        .is_empty());
    }

    #[test]
    fn pr6_work_unit_context_and_actions_exclude_sibling_scope_data() {
        assert!(work_unit_context_matches(Some("wu-a"), Some("wu-a")));
        assert!(!work_unit_context_matches(Some("wu-a"), Some("wu-b")));
        assert!(!work_unit_context_matches(Some("wu-a"), None));
        assert!(work_unit_context_matches(None, Some("wu-b")));
        assert!(work_unit_proposal_matches(
            Some("wu-a"),
            ProposalTargetKind::WorkUnit,
            "wu-a",
        ));
        assert!(!work_unit_proposal_matches(
            Some("wu-a"),
            ProposalTargetKind::WorkUnit,
            "wu-b",
        ));
        assert!(!work_unit_proposal_matches(
            Some("wu-a"),
            ProposalTargetKind::Task,
            "task-a",
        ));
        assert!(action_work_unit_matches(Some("wu-a"), Some("wu-a")));
        assert!(!action_work_unit_matches(Some("wu-a"), Some("wu-b")));
        assert!(action_work_unit_matches(None, Some("wu-b")));
    }

    #[test]
    fn pr6_execution_classifier_wakes_workers_but_never_orchestrators() {
        assert!(execution_lifecycle_is_orchestrator_wake(
            "execution.completed",
            "task-1",
            "task-1",
            Some(ExecutionPurpose::Implement),
        ));
        assert!(!execution_lifecycle_is_orchestrator_wake(
            "execution.completed",
            "task-1",
            "task-1",
            Some(ExecutionPurpose::Orchestrate),
        ));
        assert!(!execution_lifecycle_is_orchestrator_wake(
            "execution.log",
            "task-1",
            "task-1",
            Some(ExecutionPurpose::Implement),
        ));
        assert!(!execution_lifecycle_is_orchestrator_wake(
            "execution.failed",
            "task-1",
            "task-2",
            Some(ExecutionPurpose::Implement),
        ));
    }

    #[test]
    fn pr6_orchestrator_terminal_requires_current_status_and_handles_start_race() {
        assert!(terminal_event_matches_execution(
            "execution.completed",
            &ExecutionStatus::Completed,
        ));
        assert!(!terminal_event_matches_execution(
            "execution.completed",
            &ExecutionStatus::Running,
        ));
        assert!(terminal_event_matches_execution(
            "execution.stalled",
            &ExecutionStatus::Failed,
        ));
        assert!(!terminal_event_matches_execution(
            "execution.cancelled",
            &ExecutionStatus::Failed,
        ));

        let wake = OrchestratorWake {
            id: "wake-1".to_owned(),
            event_id: "event-1".to_owned(),
            event_sequence: 1,
            task_id: "task-1".to_owned(),
            task_role_id: "role-1".to_owned(),
            coordination_mode: Some(CoordinationMode::Collaborative),
            actor_kind: ActorKind::Agent,
            actor_id: "agent-1".to_owned(),
            work_unit_id: None,
            correlation_id: "correlation-1".to_owned(),
            causation_id: Some("event-1".to_owned()),
            causation_depth: 0,
            policy_ref: POLICY_REF.to_owned(),
            policy_version: POLICY_VERSION,
            policy_digest: current_policy_digest(),
            task_role_version: 1,
            task_role_policy_json: "{}".to_owned(),
            state: OrchestratorWakeState::Leased,
            available_at: "2026-01-01T00:00:00Z".to_owned(),
            lease_owner: Some("consumer-1".to_owned()),
            lease_until: Some("2026-01-01T00:01:00Z".to_owned()),
            attempt_count: 1,
            current_attempt: Some(1),
            last_error: None,
            version: 1,
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        let attempt = OrchestratorWakeExecution {
            wake_id: wake.id.clone(),
            attempt_number: 1,
            execution_id: "execution-1".to_owned(),
            state: "start_requested".to_owned(),
            last_error: None,
            created_at: wake.created_at.clone(),
            updated_at: wake.updated_at.clone(),
        };
        assert_eq!(
            terminal_reconciliation_state(&wake, &attempt),
            Some(OrchestratorWakeState::Leased),
        );
        assert!(wake_uses_current_policy(&wake));
        let mut stale_policy = wake.clone();
        stale_policy.policy_version += 1;
        assert!(!wake_uses_current_policy(&stale_policy));
        stale_policy.policy_version = POLICY_VERSION;
        stale_policy.policy_digest = "stale".to_owned();
        assert!(!wake_uses_current_policy(&stale_policy));

        let mut not_started = attempt;
        not_started.state = "reserved".to_owned();
        assert_eq!(terminal_reconciliation_state(&wake, &not_started), None);
    }

    #[test]
    fn pr6_execution_lifecycle_replay_keeps_durable_correlation_and_payload() {
        let execution = db::Execution {
            id: "execution-1".to_owned(),
            task_id: "task-1".to_owned(),
            agent_id: Some("agent-1".to_owned()),
            actor_kind: Some(ActorKind::Agent),
            actor_id: Some("agent-1".to_owned()),
            role: "implementer".to_owned(),
            purpose: Some(ExecutionPurpose::Implement),
            status: ExecutionStatus::Running,
            stop_reason: None,
            stopped_by: None,
            resume_policy: None,
            stopped_at: None,
            parent_execution_id: Some("parent-1".to_owned()),
            agent_session_id: None,
            harness_session_id: None,
            agent_message_id: None,
            last_activity_at: None,
            prompt: None,
            summary: None,
            logs_path: None,
            before_sha: None,
            after_sha: None,
            error: None,
            executor_config_snapshot_json: None,
            workspace_id: None,
            work_unit_id: Some("wu-1".to_owned()),
            work_unit_version: Some(1),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
            updated_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        let first = crate::task_service::execution_status_domain_event(
            &execution,
            &ExecutionStatus::Completed,
            "2026-01-01T00:01:00Z",
        );
        let replay = crate::task_service::execution_status_domain_event(
            &execution,
            &ExecutionStatus::Completed,
            "2026-01-01T00:02:00Z",
        );
        assert_eq!(first.correlation_id, replay.correlation_id);
        assert_eq!(first.causation_id, replay.causation_id);
        assert_eq!(first.causation_depth, replay.causation_depth);
        assert_eq!(first.dedupe_key, replay.dedupe_key);
        assert_eq!(first.payload_json, replay.payload_json);
    }

    #[test]
    fn pr6_causation_depth_and_self_event_guards_fail_closed() {
        assert!(wake_depth_allowed(0));
        assert!(wake_depth_allowed(14));
        assert!(!wake_depth_allowed(15));
        assert!(!wake_depth_allowed(16));
        let actor = member(ActorKind::Agent, "a");
        let event = DomainEvent {
            sequence: 1,
            id: "event-1".to_owned(),
            event_type: "message.created".to_owned(),
            entity_type: "message".to_owned(),
            entity_id: "message-1".to_owned(),
            actor_type: "agent".to_owned(),
            actor_id: Some("a".to_owned()),
            scope_type: "task".to_owned(),
            scope_id: "task-1".to_owned(),
            correlation_id: "corr-1".to_owned(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: None,
            payload_json: "{}".to_owned(),
            created_at: "2026-01-01T00:00:00Z".to_owned(),
        };
        assert!(event_is_authored_by_member(&event, &actor));
    }

    #[test]
    fn pr6_protected_actions_can_only_be_recorded_as_proposals() {
        assert!(is_protected_action("stop"));
        assert!(is_protected_action("merge"));
        assert!(is_protected_action("override"));
        assert!(!is_protected_action("message"));
        assert!(!is_protected_action("create_work_unit"));
    }

    #[test]
    fn pr6_task_role_policy_defaults_restricts_actions_and_rejects_unknown_schema() {
        let defaults = TaskRoleOrchestratorPolicy::parse("{}")
            .expect("empty policy preserves the PR6 default");
        assert!(defaults.permits_automatic_orchestration());
        assert_eq!(defaults.action_limit(), MAX_POLICY_ACTIONS);
        assert_eq!(defaults.work_unit_limit(), MAX_POLICY_WORK_UNITS);
        let message = OrchestratorAction::Message {
            target: ActionTarget::Task,
            work_unit_id: None,
            body: "status update".to_owned(),
        };
        let create_work_unit = OrchestratorAction::CreateWorkUnit {
            title: "follow-up".to_owned(),
            scope: "bounded scope".to_owned(),
            role: "implementer".to_owned(),
            parent_work_unit_id: None,
            assigned_actor: None,
            requires_integration: false,
        };
        validate_action_policy(&defaults, &[message.clone(), create_work_unit.clone()])
            .expect("default policy permits the full PR6 action set");

        let restrictive = TaskRoleOrchestratorPolicy::parse(
            r#"{"schema_version":1,"allowed_actions":["message"],"max_actions_per_execution":1,"max_work_unit_creations_per_execution":0}"#,
        )
        .expect("supported restrictive policy parses");
        assert!(restrictive.permits(&message));
        assert!(!restrictive.permits(&create_work_unit));
        assert!(validate_action_policy(&restrictive, &[message.clone()]).is_ok());
        assert!(validate_action_policy(&restrictive, &[create_work_unit]).is_err());
        assert!(validate_action_policy(&restrictive, &[message.clone(), message]).is_err());
        let agent_dispatch_disabled =
            TaskRoleOrchestratorPolicy::parse(r#"{"automatic_orchestration":false}"#)
                .expect("automatic dispatch can be disabled");
        assert!(!agent_dispatch_disabled.permits_automatic_orchestration());

        assert!(TaskRoleOrchestratorPolicy::parse(
            r#"{"automatic_orchestration":true,"capacity":3}"#
        )
        .is_err());
        assert!(TaskRoleOrchestratorPolicy::parse(r#"{"schema_version":2}"#).is_err());
        assert!(TaskRoleOrchestratorPolicy::parse(r#"{"allowed_actions":["steer"]}"#).is_err());
        assert!(
            TaskRoleOrchestratorPolicy::parse(r#"{"allowed_actions":["message","message"]}"#)
                .is_err()
        );
        assert!(TaskRoleOrchestratorPolicy::parse(r#"{"max_actions_per_execution":17}"#).is_err());
    }

    #[test]
    fn pr6_direct_harness_steering_is_explicitly_unsupported() {
        assert!(serde_json::from_value::<OrchestratorResponse>(json!({
            "actions": [{
                "type": "steer",
                "execution_id": "worker-execution",
                "message": "continue with the next WorkUnit"
            }]
        }))
        .is_err());
    }
}
