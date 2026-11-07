use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use api_types::{ActorRef as ApiActorRef, TaskRoleCapability, TaskRoleConfiguration};
use db::{
    new_uuid_v4, now_rfc3339, ActorRef, AgentRepo, ArtifactKind, CollaborationRepo,
    CreateDomainEvent, RoleMembershipRepo, RoleMembershipStatus, SqliteDb, Task, TaskCoordination,
    TaskCoordinationMaterialization, TaskCoordinationMode, TaskCoordinationPlan,
    TaskCoordinationPlanState, TaskCoordinationRepo, TaskCoordinationRequest,
    TaskCoordinationRequestState, TaskCoordinationWrite, TaskIntervention, TaskInterventionKind,
    TaskInterventionState, TaskLifecycleRepo, TaskLifecycleState, TaskQualityCategory,
    TaskQualityRequest, TaskQualityState, TaskQualityTargetKind, TaskRepo, TaskRoleRepo, UserRepo,
};
use events::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    collaboration_service::{
        CollaborationActorSource, CollaborationService, CreateDecisionInput, CreateHandoffInput,
        CreateMessageInput, CreateProposalInput,
    },
    domain_event_service::DomainEventService,
    task_lifecycle::TaskLifecycleService,
    Result, ServiceError,
};

const MAX_PLAN_BYTES: usize = 256 * 1024;
const MAX_BRIEF_BYTES: usize = 64 * 1024;
const MAX_PLAN_WORK_UNITS: usize = 4;
const MAX_TITLE_BYTES: usize = 512;
const MAX_SCOPE_BYTES: usize = 4096;
const MAX_INSTRUCTIONS_BYTES: usize = 64 * 1024;
const MAX_ACCEPTANCE_ITEMS: usize = 32;
const MAX_DEPENDENCIES: usize = 32;
const MAX_QUALITY_SUMMARY_BYTES: usize = 8192;
const MAX_QUALITY_FINDINGS: usize = 64;
const MAX_QUALITY_FINDING_BYTES: usize = 4096;
const MAX_RESOLUTION_BYTES: usize = 4096;
const MAX_INSTRUCTION_BYTES: usize = 16 * 1024;

fn coordination_error(code: &'static str, message: impl Into<String>) -> ServiceError {
    ServiceError::Coordination {
        code,
        message: message.into(),
    }
}

fn invalid_plan(message: impl Into<String>) -> ServiceError {
    coordination_error("coordination.invalid_plan", message)
}

fn unsupported_item(message: impl Into<String>) -> ServiceError {
    coordination_error("coordination.unsupported_plan_item", message)
}

fn sha256_hex(value: &[u8]) -> String {
    hex::encode(Sha256::digest(value))
}

fn canonical_json<T: Serialize>(value: &T) -> Result<String> {
    serde_json::to_string(value).map_err(|error| {
        ServiceError::invalid_operation(format!("canonical JSON serialization failed: {error}"))
    })
}

fn bounded(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CoordinationEnvelope {
    pub schema_version: u32,
    pub max_work_units: usize,
    pub allow_new_task_roles: bool,
    pub allow_membership_changes: bool,
    pub allow_capability_changes: bool,
    pub allow_repository_write: bool,
    pub allow_external_actions: bool,
    pub allow_integration: bool,
    pub allow_merge: bool,
    pub allowed_review_roles: Vec<String>,
    pub require_independent_review: bool,
}

impl Default for CoordinationEnvelope {
    fn default() -> Self {
        Self {
            schema_version: 1,
            max_work_units: MAX_PLAN_WORK_UNITS,
            allow_new_task_roles: false,
            allow_membership_changes: false,
            allow_capability_changes: false,
            allow_repository_write: true,
            allow_external_actions: false,
            allow_integration: true,
            allow_merge: false,
            allowed_review_roles: vec!["reviewer".to_owned()],
            require_independent_review: true,
        }
    }
}

impl CoordinationEnvelope {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            return Err(unsupported_item(
                "only envelope schema version 1 is supported",
            ));
        }
        if self.max_work_units == 0 || self.max_work_units > MAX_PLAN_WORK_UNITS {
            return Err(unsupported_item(format!(
                "max_work_units must be between 1 and {MAX_PLAN_WORK_UNITS}"
            )));
        }
        if self.allow_new_task_roles
            || self.allow_membership_changes
            || self.allow_capability_changes
        {
            return Err(unsupported_item(
                "v1 coordination cannot grant TaskRole, membership, or capability changes",
            ));
        }
        if self.allow_merge || self.allow_external_actions {
            return Err(unsupported_item(
                "v1 coordination cannot grant merge or external actions",
            ));
        }
        if self.allowed_review_roles.is_empty() {
            return Err(unsupported_item("allowed_review_roles must not be empty"));
        }
        let mut unique = HashSet::new();
        for role in &self.allowed_review_roles {
            let Some(role) = db::canonical_task_role_name(role) else {
                return Err(unsupported_item(
                    "allowed_review_roles contains an invalid role",
                ));
            };
            if !unique.insert(role) {
                return Err(unsupported_item(
                    "allowed_review_roles contains a duplicate",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct CoordinationBrief {
    pub objective: String,
    pub constraints: Option<String>,
    pub acceptance_criteria: Vec<String>,
    pub context: Option<String>,
}

impl CoordinationBrief {
    pub fn validate(&self) -> Result<()> {
        if !bounded(&self.objective, MAX_BRIEF_BYTES) {
            return Err(invalid_plan(
                "coordination objective must contain 1 to 65536 bytes",
            ));
        }
        if self
            .constraints
            .as_deref()
            .is_some_and(|value| value.len() > MAX_BRIEF_BYTES)
            || self
                .context
                .as_deref()
                .is_some_and(|value| value.len() > MAX_BRIEF_BYTES)
        {
            return Err(invalid_plan("coordination brief field is too large"));
        }
        if self.acceptance_criteria.len() > MAX_ACCEPTANCE_ITEMS
            || self
                .acceptance_criteria
                .iter()
                .any(|criterion| criterion.len() > MAX_SCOPE_BYTES)
        {
            return Err(invalid_plan(
                "coordination acceptance criteria exceed the v1 limits",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitPlan {
    pub schema_version: u32,
    pub request_id: String,
    pub task_id: String,
    pub task_version: i64,
    pub authorization_version: i64,
    pub team_digest: String,
    pub envelope_digest: String,
    pub objective_digest: String,
    pub work_units: Vec<PlanWorkUnit>,
    #[serde(default)]
    pub delivery: Option<Value>,
    #[serde(default)]
    pub review: Option<Value>,
    #[serde(default)]
    pub limits: Option<Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanWorkUnit {
    pub key: String,
    pub title: String,
    pub scope: String,
    pub role: String,
    #[serde(default)]
    pub assigned_actor: Option<ApiActorRef>,
    #[serde(default)]
    pub parent_key: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub requires_integration: bool,
    #[serde(default)]
    pub execution_instructions: Option<String>,
    #[serde(default)]
    pub review_subject_key: Option<String>,
    #[serde(default)]
    pub acceptance: Vec<String>,
    #[serde(default)]
    pub artifact_inputs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CoordinatorView {
    pub task_role_id: String,
    pub role: String,
    pub actor_ref: ActorRef,
    pub display_name: String,
    pub harness: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TeamMemberView {
    pub id: String,
    pub actor_ref: ActorRef,
    pub display_name: String,
    pub harness: Option<String>,
    pub status: String,
    pub version: i64,
}

#[derive(Debug, Clone)]
pub struct TeamRoleView {
    pub id: String,
    pub role: String,
    pub display_label: Option<String>,
    pub coordination_mode: Option<db::CoordinationMode>,
    pub capabilities: Vec<TaskRoleCapability>,
    pub configuration: TaskRoleConfiguration,
    pub version: i64,
    pub members: Vec<TeamMemberView>,
}

#[derive(Debug, Clone)]
pub struct CoordinationBlocker {
    pub code: String,
    pub message: String,
    pub task_role_id: Option<String>,
    pub actor_ref: Option<ActorRef>,
}

#[derive(Debug, Clone)]
pub struct PlanView {
    pub plan: TaskCoordinationPlan,
    pub parsed: WorkUnitPlan,
    pub mode: TaskCoordinationMode,
    pub materialization: Option<TaskCoordinationMaterialization>,
}

#[derive(Debug, Clone)]
pub struct Overview {
    pub task: Task,
    pub coordination: TaskCoordination,
    pub envelope: CoordinationEnvelope,
    pub team: Vec<TeamRoleView>,
    pub coordinator: Option<CoordinatorView>,
    pub blockers: Vec<CoordinationBlocker>,
    pub can_authorize: bool,
    pub can_request: bool,
    pub latest_request: Option<(TaskCoordinationRequest, CoordinationBrief)>,
    pub latest_plan: Option<PlanView>,
}

#[derive(Debug, Clone)]
pub struct AuthorizationInput {
    pub expected_task_version: i64,
    pub expected_coordination_version: i64,
    pub mode: TaskCoordinationMode,
    pub coordinator_task_role_id: Option<String>,
    pub coordinator_actor: Option<ActorRef>,
    pub envelope: Option<Value>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct TurnRequestInput {
    pub expected_task_version: i64,
    pub expected_coordination_version: i64,
    pub brief: CoordinationBrief,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct PlanDecisionInput {
    pub expected_coordination_version: i64,
    pub expected_plan_digest: String,
    pub approve: bool,
    pub rationale: Option<String>,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct MaterializeInput {
    pub expected_task_version: i64,
    pub expected_coordination_version: i64,
    pub expected_plan_digest: String,
    pub expected_plan_version: i64,
    pub expected_request_version: i64,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct MaterializeOutcome {
    pub plan: PlanView,
    pub request: TaskCoordinationRequest,
    pub materialization: TaskCoordinationMaterialization,
    pub replayed: bool,
    pub launch_intent_accepted: bool,
    pub launch_blockers: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct QualityRequestInput {
    pub source_artifact_id: Option<String>,
    pub category: TaskQualityCategory,
    pub target_kind: TaskQualityTargetKind,
    pub target_task_role_id: Option<String>,
    pub target_actor: Option<ActorRef>,
    pub summary: String,
    pub findings: Vec<String>,
    pub resolution_condition: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone)]
pub struct InterventionInput {
    pub kind: TaskInterventionKind,
    pub expected_task_version: Option<i64>,
    pub expected_coordination_version: Option<i64>,
    pub expected_lifecycle_version: Option<i64>,
    pub instruction: Option<String>,
    pub plan_artifact_id: Option<String>,
    pub approve: Option<bool>,
    pub rationale: Option<String>,
    pub reason: Option<String>,
    pub brief: Option<TurnRequestInput>,
    pub idempotency_key: String,
}

#[derive(Clone)]
pub struct TaskCoordinationService {
    db: Arc<SqliteDb>,
    event_bus: Arc<EventBus>,
    collaboration: CollaborationService,
    domain_events: DomainEventService,
}

impl TaskCoordinationService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            collaboration: CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            domain_events: DomainEventService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            db,
            event_bus,
        }
    }

    pub async fn overview(&self, task_id: &str, user_id: &str) -> Result<Overview> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_source(task_id, &source)
            .await?;
        let coordination = self.require_coordination(&task.id).await?;
        let envelope = self.envelope(&coordination)?;
        let team = self.current_team(&task.id).await?;
        let team_digest = self.team_digest(&team)?;
        let coordinator = self.resolve_coordinator(&coordination, &team).await?;
        let project = db::ProjectRepo::get_by_id(&*self.db, &task.project_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("project", task.project_id.clone()))?;
        let mutable =
            crate::project_authority::user_can_write_project(&self.db, &project, user_id).await?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        let dispatch = TaskLifecycleRepo::get_task_dispatch(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task dispatch", task.id.clone()))?;
        let mut blockers = self
            .coordination_blockers(
                &task,
                &coordination,
                &team,
                coordinator.as_ref(),
                &team_digest,
            )
            .await?;
        if lifecycle.state == TaskLifecycleState::Cancelled {
            blockers.push(CoordinationBlocker {
                code: "coordination.task_cancelled".to_owned(),
                message: "the Task is cancelled".to_owned(),
                task_role_id: None,
                actor_ref: None,
            });
        }
        if dispatch.state == db::TaskDispatchState::Paused {
            blockers.push(CoordinationBlocker {
                code: "coordination.paused".to_owned(),
                message: "the Task is paused; materialization and launch wait for resume"
                    .to_owned(),
                task_role_id: None,
                actor_ref: None,
            });
        }
        let requests =
            TaskCoordinationRepo::list_task_coordination_requests(&*self.db, &task.id, 1).await?;
        let latest_request = match requests.into_iter().next() {
            Some(request) => Some((request.clone(), self.parse_brief(&request)?)),
            None => None,
        };
        let plans =
            TaskCoordinationRepo::list_task_coordination_plans(&*self.db, &task.id, 1).await?;
        let latest_plan = match plans.into_iter().next() {
            Some(plan) => Some(self.plan_view(plan).await?),
            None => None,
        };
        let can_request = mutable
            && coordination.mode != TaskCoordinationMode::Manual
            && coordination.revoked_at.is_none()
            && lifecycle.state != TaskLifecycleState::Cancelled
            && dispatch.state != db::TaskDispatchState::Cancelled
            && coordinator
                .as_ref()
                .is_some_and(|coordinator| coordinator.actor_ref.kind() == db::ActorKind::Agent)
            && blockers.iter().all(|blocker| {
                !matches!(
                    blocker.code.as_str(),
                    "coordination.coordinator_missing"
                        | "coordination.coordinator_not_member"
                        | "coordination.coordinator_capability_missing"
                        | "coordination.authorization_revoked"
                )
            })
            && coordination.turn_count < coordination.max_turns;
        Ok(Overview {
            task,
            coordination,
            envelope,
            team,
            coordinator,
            blockers,
            can_authorize: mutable,
            can_request,
            latest_request,
            latest_plan,
        })
    }

    pub async fn authorize(
        &self,
        task_id: &str,
        user_id: &str,
        input: AuthorizationInput,
    ) -> Result<Overview> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
            return Err(invalid_plan("idempotency key must contain 1 to 256 bytes"));
        }
        let envelope = match input.envelope.as_ref() {
            Some(value) => {
                serde_json::from_value::<CoordinationEnvelope>(value.clone()).map_err(|error| {
                    unsupported_item(format!("invalid coordination envelope: {error}"))
                })?
            }
            None => CoordinationEnvelope::default(),
        };
        envelope.validate()?;
        if input.mode != TaskCoordinationMode::Manual
            && (input.coordinator_task_role_id.is_none() || input.coordinator_actor.is_none())
        {
            return Err(invalid_plan(
                "proposal and prepare_and_execute require an exact coordinator TaskRole and Actor",
            ));
        }
        let team = self.current_team(&task.id).await?;
        let team_snapshot_json = self.team_snapshot_json(&team)?;
        let team_digest = sha256_hex(team_snapshot_json.as_bytes());
        let coordinator = match (
            input.coordinator_task_role_id.as_deref(),
            input.coordinator_actor.as_ref(),
        ) {
            (Some(role_id), Some(actor)) => {
                let role = team.iter().find(|role| role.id == role_id).ok_or_else(|| {
                    coordination_error(
                        "coordination.not_authorized",
                        "coordinator TaskRole does not belong to this Task",
                    )
                })?;
                if !role
                    .capabilities
                    .contains(&TaskRoleCapability::Orchestration)
                {
                    return Err(coordination_error(
                        "coordination.not_authorized",
                        "coordinator TaskRole does not have orchestration capability",
                    ));
                }
                self.require_active_member(role, actor).await?;
                self.require_actor_exists(actor).await?;
                Some(actor.clone())
            }
            (None, None) => None,
            _ => {
                return Err(invalid_plan(
                    "coordinator TaskRole and Actor must be provided together",
                ))
            }
        };
        let envelope_json = canonical_json(&envelope)?;
        let envelope_digest = sha256_hex(envelope_json.as_bytes());
        let request_digest = sha256_hex(
            canonical_json(&json!({
                "operation": "authorize",
                "task_id": task.id,
                "mode": input.mode.to_string(),
                "coordinator_task_role_id": input.coordinator_task_role_id,
                "coordinator_actor": coordinator,
                "team_digest": team_digest,
                "envelope_digest": envelope_digest,
                "requested_by": user_id,
            }))?
            .as_bytes(),
        );
        let now = now_rfc3339();
        let event = self.event(
            "task.coordination_authorized",
            "task_coordination",
            &task.id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "mode": input.mode.to_string(),
                "authorization_version": input.expected_coordination_version + 1,
                "team_digest": team_digest,
                "envelope_digest": envelope_digest,
            }),
            &now,
        );
        let write = TaskCoordinationRepo::authorize_task_coordination(
            &*self.db,
            db::AuthorizeTaskCoordinationInput {
                task_id: task.id.clone(),
                expected_coordination_version: input.expected_coordination_version,
                mode: input.mode,
                coordinator_task_role_id: input.coordinator_task_role_id,
                coordinator_actor: coordinator,
                team_snapshot_json,
                team_digest,
                envelope_json,
                envelope_digest,
                requested_by: user_id.to_owned(),
                idempotency_key: input.idempotency_key,
                request_digest,
                receipt_id: new_uuid_v4(),
                created_at: now,
                event,
            },
            input.expected_task_version,
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        self.overview(task_id, user_id).await
    }

    pub async fn revoke(
        &self,
        task_id: &str,
        user_id: &str,
        expected_task_version: i64,
        expected_coordination_version: i64,
        reason: Option<String>,
        idempotency_key: &str,
    ) -> Result<Overview> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        let request_digest = sha256_hex(
            canonical_json(&json!({
                "operation": "revoke",
                "task_id": task.id,
                "reason": reason,
                "requested_by": user_id,
            }))?
            .as_bytes(),
        );
        let now = now_rfc3339();
        let event = self.event(
            "task.coordination_revoked",
            "task_coordination",
            &task.id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "authorization_version": expected_coordination_version + 1,
            }),
            &now,
        );
        let write: TaskCoordinationWrite = TaskCoordinationRepo::revoke_task_coordination(
            &*self.db,
            db::RevokeTaskCoordinationInput {
                task_id: task.id,
                expected_coordination_version,
                reason,
                requested_by: user_id.to_owned(),
                idempotency_key: idempotency_key.to_owned(),
                request_digest,
                receipt_id: new_uuid_v4(),
                created_at: now,
                event,
            },
            expected_task_version,
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        self.overview(task_id, user_id).await
    }

    pub async fn request_turn(
        &self,
        task_id: &str,
        user_id: &str,
        input: TurnRequestInput,
    ) -> Result<(TaskCoordinationRequest, bool)> {
        input.brief.validate()?;
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
            return Err(invalid_plan("idempotency key must contain 1 to 256 bytes"));
        }
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        if task.version != input.expected_task_version {
            return Err(db::DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task.version,
            }
            .into());
        }
        let coordination = self.require_coordination(&task.id).await?;
        if coordination.authorization_version != input.expected_coordination_version {
            return Err(coordination_error(
                "coordination.version_conflict",
                "Task coordination authorization changed",
            ));
        }
        if coordination.mode == TaskCoordinationMode::Manual {
            return Err(coordination_error(
                "coordination.not_authorized",
                "Task coordination is in manual mode",
            ));
        }
        if coordination.revoked_at.is_some() {
            return Err(coordination_error(
                "coordination.authorization_revoked",
                "Task coordination authorization is revoked",
            ));
        }
        if let Some(request) = TaskCoordinationRepo::get_task_coordination_request_by_key(
            &*self.db,
            &task.id,
            &input.idempotency_key,
        )
        .await?
        {
            let brief = self.parse_brief(&request)?;
            let request_digest = self.turn_request_digest(&task, &coordination, &brief)?;
            if request.request_digest != request_digest {
                return Err(coordination_error(
                    "coordination.idempotency_conflict",
                    "idempotency key already identifies a different coordination request",
                ));
            }
            return Ok((request, false));
        }
        let team = self.current_team(&task.id).await?;
        if self.team_digest(&team)? != coordination.team_digest {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "Task team changed since the coordination authorization; reauthorize it",
            ));
        }
        let coordinator = self
            .resolve_coordinator(&coordination, &team)
            .await?
            .ok_or_else(|| {
                coordination_error(
                    "coordination.coordinator_missing",
                    "Task coordination has no eligible coordinator",
                )
            })?;
        if coordinator.actor_ref.kind() != db::ActorKind::Agent {
            return Err(coordination_error(
                "coordination.human_turn_unsupported",
                "a Human coordinator edits the plan manually; an automatic turn requires an Agent",
            ));
        }
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if matches!(
            lifecycle.state,
            TaskLifecycleState::Cancelled | TaskLifecycleState::Done | TaskLifecycleState::Merging
        ) {
            return Err(coordination_error(
                "coordination.not_authorized",
                "coordination turns are not available in the current TaskLifecycle state",
            ));
        }
        let dispatch = TaskLifecycleRepo::get_task_dispatch(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task dispatch", task.id.clone()))?;
        if dispatch.state == db::TaskDispatchState::Cancelled {
            return Err(coordination_error(
                "coordination.not_authorized",
                "Task dispatch is cancelled",
            ));
        }
        if !TaskCoordinationRepo::consume_coordination_turn_budget(&*self.db, &task.id).await? {
            return Err(coordination_error(
                "coordination.budget_exhausted",
                "the Task coordination turn budget is exhausted",
            ));
        }
        let brief_json = canonical_json(&input.brief)?;
        let objective_digest = sha256_hex(brief_json.as_bytes());
        let request_digest = self.turn_request_digest(&task, &coordination, &input.brief)?;
        let request_id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
            "task.coordination_requested",
            "task_coordination_request",
            &request_id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "request_id": request_id,
                "mode": coordination.mode.to_string(),
                "authorization_version": coordination.authorization_version,
                "objective_digest": objective_digest,
            }),
            &now,
        );
        let (request, committed) = TaskCoordinationRepo::create_task_coordination_request(
            &*self.db,
            db::CreateTaskCoordinationRequestInput {
                id: request_id,
                task_id: task.id.clone(),
                authorization_version: coordination.authorization_version,
                mode: coordination.mode,
                brief_json,
                objective_digest,
                request_digest,
                idempotency_key: input.idempotency_key,
                created_at: now,
                event,
            },
        )
        .await?;
        if committed.is_none() {
            TaskCoordinationRepo::refund_coordination_turn_budget(&*self.db, &task.id).await?;
        } else if let Some(event) = committed.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok((request, committed.is_some()))
    }

    pub async fn get_request(
        &self,
        task_id: &str,
        request_id: &str,
        user_id: &str,
    ) -> Result<(TaskCoordinationRequest, CoordinationBrief)> {
        self.collaboration
            .authorize_source(
                task_id,
                &CollaborationActorSource::Human(user_id.to_owned()),
            )
            .await?;
        let request = TaskCoordinationRepo::get_task_coordination_request(&*self.db, request_id)
            .await?
            .filter(|request| request.task_id == task_id)
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_request", request_id.to_owned())
            })?;
        let brief = self.parse_brief(&request)?;
        Ok((request, brief))
    }

    pub async fn get_plan(
        &self,
        task_id: &str,
        artifact_id: &str,
        user_id: &str,
    ) -> Result<PlanView> {
        self.collaboration
            .authorize_source(
                task_id,
                &CollaborationActorSource::Human(user_id.to_owned()),
            )
            .await?;
        let plan = TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id)
            .await?
            .filter(|plan| plan.task_id == task_id)
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_plan", artifact_id.to_owned())
            })?;
        self.plan_view(plan).await
    }

    pub async fn approve_plan(
        &self,
        task_id: &str,
        artifact_id: &str,
        user_id: &str,
        input: PlanDecisionInput,
    ) -> Result<PlanView> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        let coordination = self.require_coordination(&task.id).await?;
        if coordination.authorization_version != input.expected_coordination_version {
            return Err(coordination_error(
                "coordination.version_conflict",
                "Task coordination authorization changed",
            ));
        }
        let plan = TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id)
            .await?
            .filter(|plan| plan.task_id == task.id)
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_plan", artifact_id.to_owned())
            })?;
        if plan.plan_digest != input.expected_plan_digest {
            return Err(coordination_error(
                "coordination.plan_digest_mismatch",
                "the plan digest does not match the inspected plan",
            ));
        }
        if plan.state == TaskCoordinationPlanState::Materialized {
            if input.approve {
                return self.plan_view(plan).await;
            }
            return Err(coordination_error(
                "coordination.already_materialized",
                "a materialized plan cannot be rejected",
            ));
        }
        if matches!(
            plan.state,
            TaskCoordinationPlanState::Rejected | TaskCoordinationPlanState::Superseded
        ) {
            if !input.approve && plan.state == TaskCoordinationPlanState::Rejected {
                return self.plan_view(plan).await;
            }
            return Err(coordination_error(
                "coordination.version_conflict",
                "the plan is no longer open for this decision",
            ));
        }
        if plan.state == TaskCoordinationPlanState::Approved && input.approve {
            return self.plan_view(plan).await;
        }
        let decision_id = if let Some(proposal_id) = plan.proposal_id.as_deref() {
            let proposal = self
                .collaboration
                .get_proposal(proposal_id, source.clone())
                .await?;
            if proposal.target_digest.as_deref() != Some(plan.plan_digest.as_str()) {
                return Err(coordination_error(
                    "coordination.plan_digest_mismatch",
                    "the Proposal no longer pins this exact plan digest",
                ));
            }
            let decision = self
                .collaboration
                .record_decision(
                    CreateDecisionInput {
                        task_id: task.id.clone(),
                        proposal_id: proposal_id.to_owned(),
                        proposal_version: proposal.content_version,
                        outcome: if input.approve {
                            db::DecisionOutcome::Approve
                        } else {
                            db::DecisionOutcome::Reject
                        },
                        rationale: input.rationale.unwrap_or_else(|| {
                            if input.approve {
                                "Approved through the Task coordination circuit".to_owned()
                            } else {
                                "Rejected through the Task coordination circuit".to_owned()
                            }
                        }),
                        policy_ref: None,
                        policy_version: None,
                        policy_digest: None,
                    },
                    vec![source],
                )
                .await?;
            Some(decision.id)
        } else {
            None
        };
        let next_state = if input.approve {
            TaskCoordinationPlanState::Approved
        } else {
            TaskCoordinationPlanState::Rejected
        };
        let now = now_rfc3339();
        let event = self.event(
            if input.approve {
                "task.coordination_plan_approved"
            } else {
                "task.coordination_plan_rejected"
            },
            "task_coordination_plan",
            artifact_id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "plan_artifact_id": artifact_id,
                "plan_digest": plan.plan_digest,
                "state": next_state.to_string(),
                "decision_id": decision_id,
            }),
            &now,
        );
        let plan = TaskCoordinationRepo::update_task_coordination_plan_state(
            &*self.db,
            artifact_id,
            plan.version,
            next_state,
            None,
            Some(decision_id),
            &now,
            Some(event),
        )
        .await?;
        self.plan_view(plan).await
    }

    pub async fn materialize(
        &self,
        task_id: &str,
        artifact_id: &str,
        user_id: &str,
        input: MaterializeInput,
    ) -> Result<MaterializeOutcome> {
        if input.idempotency_key.trim().is_empty() {
            return Err(invalid_plan("idempotency key must not be empty"));
        }
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        if let Some(existing) =
            TaskCoordinationRepo::get_task_coordination_materialization(&*self.db, artifact_id)
                .await?
        {
            if existing.plan_digest != input.expected_plan_digest {
                return Err(coordination_error(
                    "coordination.plan_digest_mismatch",
                    "the materialized plan digest does not match",
                ));
            }
            let plan = TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("task_coordination_plan", artifact_id.to_owned())
                })?;
            let request = TaskCoordinationRepo::get_task_coordination_request(
                &*self.db,
                &existing.request_id,
            )
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_request", existing.request_id.clone())
            })?;
            return Ok(MaterializeOutcome {
                plan: self.plan_view(plan).await?,
                request,
                materialization: existing,
                replayed: true,
                launch_intent_accepted: false,
                launch_blockers: Vec::new(),
            });
        }
        let plan = TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id)
            .await?
            .filter(|plan| plan.task_id == task.id)
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_plan", artifact_id.to_owned())
            })?;
        if plan.state == TaskCoordinationPlanState::Proposed {
            return Err(coordination_error(
                "coordination.plan_not_approved",
                "the plan requires an explicit Human approval",
            ));
        }
        if !matches!(
            plan.state,
            TaskCoordinationPlanState::Approved | TaskCoordinationPlanState::Materialized
        ) {
            return Err(coordination_error(
                "coordination.version_conflict",
                "the plan is not approved for materialization",
            ));
        }
        let coordination = self.require_coordination(&task.id).await?;
        if coordination.authorization_version != input.expected_coordination_version
            || plan.authorization_version != input.expected_coordination_version
        {
            return Err(coordination_error(
                "coordination.version_conflict",
                "Task coordination authorization changed",
            ));
        }
        if coordination.revoked_at.is_some() {
            return Err(coordination_error(
                "coordination.authorization_revoked",
                "Task coordination authorization is revoked",
            ));
        }
        let request =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &plan.request_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("task_coordination_request", plan.request_id.clone())
                })?;
        let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
            .await?
            .filter(|artifact| artifact.task_id == task.id)
            .ok_or_else(|| ServiceError::not_found("artifact", artifact_id.to_owned()))?;
        let content = artifact
            .content
            .as_deref()
            .ok_or_else(|| invalid_plan("the plan Artifact is not inline JSON"))?;
        if content.len() > MAX_PLAN_BYTES
            || artifact.digest.as_deref() != Some(plan.plan_digest.as_str())
            || sha256_hex(content.as_bytes()) != plan.plan_digest
        {
            return Err(coordination_error(
                "coordination.plan_digest_mismatch",
                "the persisted plan Artifact no longer matches its approved digest",
            ));
        }
        let parsed = self.parse_plan(content)?;
        let team = self.current_team(&task.id).await?;
        let current_team_digest = self.team_digest(&team)?;
        let envelope = self.envelope(&coordination)?;
        self.validate_plan(
            &task,
            &coordination,
            &envelope,
            &team,
            &current_team_digest,
            &request,
            &parsed,
            true,
        )
        .await?;
        if task.version != input.expected_task_version || parsed.task_version != task.version {
            return Err(db::DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task.version,
            }
            .into());
        }
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        if matches!(
            lifecycle.state,
            TaskLifecycleState::Cancelled | TaskLifecycleState::Done | TaskLifecycleState::Merging
        ) {
            return Err(coordination_error(
                "coordination.not_authorized",
                "materialization is not available in the current TaskLifecycle state",
            ));
        }
        let dispatch = TaskLifecycleRepo::get_task_dispatch(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task dispatch", task.id.clone()))?;
        if dispatch.state == db::TaskDispatchState::Paused {
            return Err(coordination_error(
                "coordination.paused",
                "the Task is paused; resume before materializing new work",
            ));
        }
        if dispatch.state == db::TaskDispatchState::Cancelled {
            return Err(coordination_error(
                "coordination.not_authorized",
                "Task dispatch is cancelled",
            ));
        }
        if !self.is_within_envelope(&envelope, &parsed) {
            return Err(coordination_error(
                "coordination.unsupported_plan_item",
                "the plan exceeds the authorized envelope; approve it explicitly first",
            ));
        }
        let work_units = self
            .build_materialized_work_units(&task, &parsed, &team)
            .await?;
        let receipt_id = new_uuid_v4();
        let work_unit_ids = work_units
            .iter()
            .map(|unit| unit.id.clone())
            .collect::<Vec<_>>();
        let receipt_digest = sha256_hex(
            canonical_json(&json!({
                "task_id": task.id,
                "request_id": request.id,
                "plan_artifact_id": artifact_id,
                "plan_digest": plan.plan_digest,
                "authorization_version": coordination.authorization_version,
                "task_version_before": task.version,
                "work_unit_ids": work_unit_ids,
                "created_by": user_id,
            }))?
            .as_bytes(),
        );
        let now = now_rfc3339();
        let correlation_id = new_uuid_v4();
        let mut events = Vec::with_capacity(work_units.len() + 1);
        for unit in &work_units {
            events.push(self.event_with_correlation(
                "work_unit.created",
                "work_unit",
                &unit.id,
                "human",
                Some(user_id),
                &task.id,
                json!({
                    "task_id": task.id,
                    "work_unit_id": unit.id,
                    "version": 1,
                    "status": "open",
                    "coordination_request_id": request.id,
                    "plan_artifact_id": artifact_id,
                }),
                &now,
                &correlation_id,
            ));
        }
        events.push(self.event_with_correlation(
            "task.coordination_plan_materialized",
            "task_coordination_plan",
            artifact_id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "plan_artifact_id": artifact_id,
                "request_id": request.id,
                "plan_digest": plan.plan_digest,
                "receipt_id": receipt_id,
                "work_unit_ids": work_unit_ids,
            }),
            &now,
            &correlation_id,
        ));
        let materialization = TaskCoordinationRepo::materialize_task_coordination_plan(
            &*self.db,
            db::MaterializeTaskCoordinationPlanInput {
                receipt_id,
                task_id: task.id.clone(),
                request_id: request.id.clone(),
                plan_artifact_id: artifact_id.to_owned(),
                plan_digest: plan.plan_digest.clone(),
                authorization_version: coordination.authorization_version,
                expected_plan_version: plan.version,
                task_version_before: task.version,
                expected_request_version: request.version,
                work_units,
                receipt_digest,
                created_by: user_id.to_owned(),
                created_at: now.clone(),
                events,
            },
        )
        .await?;
        let plan = TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_plan", artifact_id.to_owned())
            })?;
        let request = TaskCoordinationRepo::get_task_coordination_request(&*self.db, &request.id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("task_coordination_request", request.id.clone())
            })?;
        let (launch_intent_accepted, launch_blockers) =
            self.launch_materialized_work(&task.id, user_id).await?;
        Ok(MaterializeOutcome {
            plan: self.plan_view(plan).await?,
            request,
            materialization,
            replayed: false,
            launch_intent_accepted,
            launch_blockers,
        })
    }

    async fn launch_materialized_work(
        &self,
        task_id: &str,
        user_id: &str,
    ) -> Result<(bool, Vec<String>)> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
        let lifecycle_service =
            TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let preflight = lifecycle_service.launch_preflight(task_id).await?;
        let blockers = preflight
            .blockers
            .iter()
            .map(|blocker| format!("{}: {}", blocker.code, blocker.message))
            .collect::<Vec<_>>();
        if !preflight.can_launch {
            return Ok((false, blockers));
        }
        match lifecycle_service
            .launch_task(
                task_id,
                task.version,
                lifecycle.version,
                &ApiActorRef::Human(user_id.to_owned()),
                &format!("coordination-launch:{task_id}:{}", lifecycle.version),
            )
            .await
        {
            Ok(write) => Ok((
                write.dispatch.state == db::TaskDispatchState::Running,
                blockers,
            )),
            Err(error) => {
                let mut blockers = blockers;
                blockers.push(error.to_string());
                Ok((false, blockers))
            }
        }
    }

    /// Revalidate exact team, authorization, and coordinator before dispatch.
    pub async fn validate_turn_authority(&self, request: &TaskCoordinationRequest) -> Result<()> {
        let coordination = self.require_coordination(&request.task_id).await?;
        if coordination.revoked_at.is_some()
            || coordination.authorization_version != request.authorization_version
            || coordination.mode != request.mode
        {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "the coordination authorization changed or was revoked",
            ));
        }
        let team = self.current_team(&request.task_id).await?;
        if self.team_digest(&team)? != coordination.team_digest {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "the Task team changed after the coordination authorization",
            ));
        }
        let coordinator = self
            .resolve_coordinator(&coordination, &team)
            .await?
            .ok_or_else(|| {
                coordination_error(
                    "coordination.authorization_stale",
                    "the selected coordinator is no longer eligible",
                )
            })?;
        if coordinator.actor_ref.kind() != db::ActorKind::Agent {
            return Err(coordination_error(
                "coordination.human_turn_unsupported",
                "an automatic coordination turn requires an Agent coordinator",
            ));
        }
        Ok(())
    }

    pub async fn complete_turn(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
    ) -> Result<()> {
        if let Some(existing) =
            TaskCoordinationRepo::list_task_coordination_plans(&*self.db, &request.task_id, 100)
                .await?
                .into_iter()
                .find(|plan| plan.request_id == request.id)
        {
            if existing.state == TaskCoordinationPlanState::Approved {
                let coordination = self.require_coordination(&request.task_id).await?;
                let task_version = TaskRepo::get_by_id(&*self.db, &request.task_id, false)
                    .await?
                    .map(|task| task.version)
                    .unwrap_or_default();
                if let Some(authorizer) = coordination.requested_by.as_deref() {
                    self.materialize(
                        &request.task_id,
                        &existing.artifact_id,
                        authorizer,
                        MaterializeInput {
                            expected_task_version: task_version,
                            expected_coordination_version: existing.authorization_version,
                            expected_plan_digest: existing.plan_digest.clone(),
                            expected_plan_version: existing.version,
                            expected_request_version: request.version,
                            idempotency_key: format!("coordination-auto:{}", existing.artifact_id),
                        },
                    )
                    .await
                    .ok();
                }
            }
            return Ok(());
        }
        let output = execution
            .summary
            .as_deref()
            .ok_or_else(|| invalid_plan("the coordination Execution has no plan output"))?;
        if output.len() > MAX_PLAN_BYTES {
            return Err(invalid_plan("the coordination plan exceeds 256 KiB"));
        }
        let parsed = self.parse_plan(output)?;
        let coordination = self.require_coordination(&request.task_id).await?;
        let team = self.current_team(&request.task_id).await?;
        let current_team_digest = self.team_digest(&team)?;
        let envelope = self.envelope(&coordination)?;
        let task = TaskRepo::get_by_id(&*self.db, &request.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", request.task_id.clone()))?;
        self.validate_plan(
            &task,
            &coordination,
            &envelope,
            &team,
            &current_team_digest,
            request,
            &parsed,
            false,
        )
        .await?;
        let within_envelope = self.is_within_envelope(&envelope, &parsed);
        let plan_digest = sha256_hex(output.as_bytes());
        let artifact_id = new_uuid_v4();
        let plan_state =
            if request.mode == TaskCoordinationMode::PrepareAndExecute && within_envelope {
                TaskCoordinationPlanState::Approved
            } else {
                TaskCoordinationPlanState::Proposed
            };
        let now = now_rfc3339();
        let artifact_event = self.event(
            "artifact.created",
            "artifact",
            &artifact_id,
            "agent",
            Some(execution.actor_id.as_deref().unwrap_or_default()),
            &request.task_id,
            json!({
                "artifact_id": artifact_id,
                "task_id": request.task_id,
                "kind": ArtifactKind::WorkUnitPlan.to_string(),
                "request_id": request.id,
                "digest": plan_digest,
            }),
            &now,
        );
        let plan_event = self.event(
            if plan_state == TaskCoordinationPlanState::Approved {
                "task.coordination_plan_approved"
            } else {
                "task.coordination_plan_proposed"
            },
            "task_coordination_plan",
            &artifact_id,
            "agent",
            execution.actor_id.as_deref(),
            &request.task_id,
            json!({
                "task_id": request.task_id,
                "plan_artifact_id": artifact_id,
                "request_id": request.id,
                "plan_digest": plan_digest,
                "state": plan_state.to_string(),
            }),
            &now,
        );
        let mut write = TaskCoordinationRepo::complete_task_coordination_turn(
            &*self.db,
            db::CompleteTaskCoordinationTurnInput {
                request_id: request.id.clone(),
                task_id: request.task_id.clone(),
                authorization_version: coordination.authorization_version,
                artifact_id: artifact_id.clone(),
                plan_digest: plan_digest.clone(),
                artifact_content: output.to_owned(),
                artifact_metadata_json: json!({
                    "schema_version": 1,
                    "task_id": request.task_id,
                    "request_id": request.id,
                    "plan_digest": plan_digest,
                    "team_digest": coordination.team_digest,
                    "envelope_digest": coordination.envelope_digest,
                    "objective_digest": request.objective_digest,
                })
                .to_string(),
                producer_execution_id: execution.id.clone(),
                producer_actor: execution
                    .actor_ref()
                    .ok_or_else(|| invalid_plan("the coordination Execution has no Actor"))?,
                plan_state,
                proposal_id: None,
                request_state: TaskCoordinationRequestState::PlanReady,
                created_at: now.clone(),
                artifact_event,
                plan_event,
            },
        )
        .await?;
        if plan_state == TaskCoordinationPlanState::Proposed {
            let proposal = self
                .collaboration
                .create_proposal_with_id(
                    CollaborationActorSource::Execution(execution.id.clone()),
                    CreateProposalInput {
                        task_id: request.task_id.clone(),
                        target: db::ProposalTarget {
                            kind: db::ProposalTargetKind::Task,
                            id: request.task_id.clone(),
                        },
                        action: "materialize_work_unit_plan".to_owned(),
                        reason: if within_envelope {
                            "Review the Task coordination plan before materializing it".to_owned()
                        } else {
                            "The Task coordination plan exceeds the authorized envelope".to_owned()
                        },
                        target_version: Some(parsed.task_version),
                        target_digest: Some(plan_digest.clone()),
                        required_policy_ref: Some("forge.task-coordination.envelope".to_owned()),
                        required_policy_version: Some(1),
                        required_policy_digest: Some(coordination.envelope_digest.clone()),
                        supersedes_proposal_id: None,
                        artifact_ids: vec![artifact_id.clone()],
                    },
                    new_uuid_v4(),
                )
                .await?;
            let plan = TaskCoordinationRepo::update_task_coordination_plan_state(
                &*self.db,
                &artifact_id,
                write.plan.version,
                TaskCoordinationPlanState::Proposed,
                Some(Some(proposal.id.clone())),
                None,
                &now,
                None,
            )
            .await?;
            let request_event = self.event(
                "task.coordination_request_state_changed",
                "task_coordination_request",
                &request.id,
                "agent",
                execution.actor_id.as_deref(),
                &request.task_id,
                json!({
                    "task_id": request.task_id,
                    "request_id": request.id,
                    "state": TaskCoordinationRequestState::AwaitingApproval.to_string(),
                    "proposal_id": proposal.id,
                }),
                &now,
            );
            let updated_request = TaskCoordinationRepo::update_task_coordination_request(
                &*self.db,
                db::UpdateTaskCoordinationRequestState {
                    id: request.id.clone(),
                    expected_version: write.request.version,
                    state: TaskCoordinationRequestState::AwaitingApproval,
                    wake_id: None,
                    execution_id: None,
                    plan_artifact_id: None,
                    failure_code: None,
                    failure_message: None,
                    completed_at: None,
                    updated_at: now,
                    event: Some(request_event),
                },
            )
            .await?;
            write.plan = plan;
            write.request = updated_request;
        } else {
            let authorizer = coordination.requested_by.clone().ok_or_else(|| {
                coordination_error(
                    "coordination.not_authorized",
                    "the coordination authorization has no Human authorizer",
                )
            })?;
            let materialize = self
                .materialize(
                    &request.task_id,
                    &artifact_id,
                    &authorizer,
                    MaterializeInput {
                        expected_task_version: parsed.task_version,
                        expected_coordination_version: coordination.authorization_version,
                        expected_plan_digest: plan_digest,
                        expected_plan_version: write.plan.version,
                        expected_request_version: write.request.version,
                        idempotency_key: format!("coordination-auto:{artifact_id}"),
                    },
                )
                .await;
            match materialize {
                Ok(_) => {}
                Err(ServiceError::Coordination { code, message })
                    if code == "coordination.paused" =>
                {
                    tracing::info!(task_id = %request.task_id, %message, "coordination plan waits for resume");
                }
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    pub async fn mark_turn_started(
        &self,
        request: &TaskCoordinationRequest,
        wake_id: &str,
        execution_id: &str,
    ) -> Result<()> {
        let now = now_rfc3339();
        let event = self.event(
            "task.coordination_turn_started",
            "task_coordination_request",
            &request.id,
            "system",
            None,
            &request.task_id,
            json!({
                "task_id": request.task_id,
                "request_id": request.id,
                "wake_id": wake_id,
                "execution_id": execution_id,
            }),
            &now,
        );
        TaskCoordinationRepo::update_task_coordination_request(
            &*self.db,
            db::UpdateTaskCoordinationRequestState {
                id: request.id.clone(),
                expected_version: request.version,
                state: TaskCoordinationRequestState::Running,
                wake_id: Some(wake_id.to_owned()),
                execution_id: Some(execution_id.to_owned()),
                plan_artifact_id: None,
                failure_code: None,
                failure_message: None,
                completed_at: None,
                updated_at: now,
                event: Some(event),
            },
        )
        .await?;
        Ok(())
    }

    pub async fn mark_request_uncertain(
        &self,
        request: &TaskCoordinationRequest,
        reason: &str,
    ) -> Result<()> {
        self.transition_request(
            request,
            TaskCoordinationRequestState::Uncertain,
            Some("coordination.provider_uncertain"),
            Some(reason),
        )
        .await
    }

    pub async fn mark_request_failed(
        &self,
        request: &TaskCoordinationRequest,
        code: &'static str,
        reason: &str,
    ) -> Result<()> {
        self.transition_request(
            request,
            TaskCoordinationRequestState::Failed,
            Some(code),
            Some(reason),
        )
        .await
    }

    pub async fn mark_request_cancelled(&self, request: &TaskCoordinationRequest) -> Result<()> {
        self.transition_request(
            request,
            TaskCoordinationRequestState::Cancelled,
            Some("coordination.cancelled"),
            Some("the Task coordination request was cancelled"),
        )
        .await
    }

    async fn transition_request(
        &self,
        request: &TaskCoordinationRequest,
        state: TaskCoordinationRequestState,
        code: Option<&'static str>,
        message: Option<&str>,
    ) -> Result<()> {
        if request.state.is_terminal() {
            return Ok(());
        }
        let now = now_rfc3339();
        let event = self.event(
            "task.coordination_request_state_changed",
            "task_coordination_request",
            &request.id,
            "system",
            None,
            &request.task_id,
            json!({
                "task_id": request.task_id,
                "request_id": request.id,
                "state": state.to_string(),
                "failure_code": code,
            }),
            &now,
        );
        TaskCoordinationRepo::update_task_coordination_request(
            &*self.db,
            db::UpdateTaskCoordinationRequestState {
                id: request.id.clone(),
                expected_version: request.version,
                state,
                wake_id: None,
                execution_id: None,
                plan_artifact_id: None,
                failure_code: Some(code.map(str::to_owned)),
                failure_message: Some(message.map(str::to_owned)),
                completed_at: state.is_terminal().then(|| now.clone()),
                updated_at: now,
                event: Some(event),
            },
        )
        .await?;
        Ok(())
    }

    pub async fn render_turn_prompt(
        &self,
        request: &TaskCoordinationRequest,
        context: &Value,
    ) -> Result<String> {
        let brief = self.parse_brief(request)?;
        let coordination = self.require_coordination(&request.task_id).await?;
        let envelope = self.envelope(&coordination)?;
        let team = self.current_team(&request.task_id).await?;
        let team_rows = team
            .iter()
            .map(|role| {
                json!({
                    "task_role_id": role.id,
                    "role": role.role,
                    "display_label": role.display_label,
                    "capabilities": role.capabilities,
                    "members": role.members.iter().map(|member| json!({
                        "actor": member.actor_ref,
                        "display_name": member.display_name,
                        "harness": member.harness,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>();
        let task_version = TaskRepo::get_by_id(&*self.db, &request.task_id, false)
            .await?
            .map(|task| task.version)
            .unwrap_or(0);
        Ok(format!(
            "You are the selected Task coordinator for one bounded preparation turn. This execution is read-only and must not modify repository files, create WorkUnits, call provider APIs, or grant authority.\n\
Return exactly one JSON object and no markdown fences. The object must match this schema:\n\
{{\"schema_version\":1,\"request_id\":{request_id},\"task_id\":{task_id},\"task_version\":{task_version},\"authorization_version\":{authorization_version},\"team_digest\":{team_digest},\"envelope_digest\":{envelope_digest},\"objective_digest\":{objective_digest},\"work_units\":[{{\"key\":\"stable-key\",\"title\":\"...\",\"scope\":\"...\",\"role\":\"exact existing TaskRole role name\",\"assigned_actor\":{{\"kind\":\"agent|human\",\"id\":\"exact eligible member id\"}}|null,\"parent_key\":null,\"depends_on\":[],\"requires_integration\":false,\"execution_instructions\":null,\"review_subject_key\":null,\"acceptance\":[],\"artifact_inputs\":[]}}],\"delivery\":null,\"review\":null,\"limits\":null}}\n\
Create at most {max_work_units} WorkUnits. Use only TaskRole names and active members listed in the team. A reviewer WorkUnit must use role `reviewer`, set review_subject_key to another planned key, and must be independent from that WorkUnit's assigned Actor. Do not create, rename, or grant TaskRoles, memberships, or capabilities. Do not request external actions or merge. A plan outside the envelope will wait for Human approval.\n\
Objective: {objective}\nConstraints: {constraints}\nAcceptance criteria: {acceptance}\nContext: {context}\nEnvelope: {envelope_json}\nTeam: {team}\nCurrent Task context: {task_context}",
            request_id = serde_json::to_string(&request.id).unwrap(),
            task_id = serde_json::to_string(&request.task_id).unwrap(),
            task_version = task_version,
            authorization_version = request.authorization_version,
            team_digest = serde_json::to_string(&coordination.team_digest).unwrap(),
            envelope_digest = serde_json::to_string(&coordination.envelope_digest).unwrap(),
            objective_digest = serde_json::to_string(&request.objective_digest).unwrap(),
            max_work_units = envelope.max_work_units,
            objective = brief.objective,
            constraints = brief.constraints.unwrap_or_else(|| "none".to_owned()),
            acceptance = serde_json::to_string(&brief.acceptance_criteria).unwrap_or_default(),
            context = brief.context.unwrap_or_else(|| "none".to_owned()),
            envelope_json = serde_json::to_string(&envelope).unwrap_or_default(),
            team = serde_json::to_string(&team_rows).unwrap_or_default(),
            task_context = serde_json::to_string(context).unwrap_or_else(|_| "{}".to_owned()),
        ))
    }

    pub async fn create_quality_request(
        &self,
        task_id: &str,
        user_id: &str,
        input: QualityRequestInput,
    ) -> Result<TaskQualityRequest> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, actor) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
            return Err(invalid_plan("quality request idempotency key is invalid"));
        }
        if !bounded(&input.summary, MAX_QUALITY_SUMMARY_BYTES) {
            return Err(invalid_plan("quality summary must contain 1 to 8192 bytes"));
        }
        if input.findings.len() > MAX_QUALITY_FINDINGS
            || input
                .findings
                .iter()
                .any(|finding| !bounded(finding, MAX_QUALITY_FINDING_BYTES))
        {
            return Err(invalid_plan("quality findings exceed the v1 limits"));
        }
        if !bounded(&input.resolution_condition, MAX_RESOLUTION_BYTES) {
            return Err(invalid_plan(
                "quality resolution condition must contain 1 to 4096 bytes",
            ));
        }
        let (source_kind, source_id, source_digest) =
            if let Some(artifact_id) = input.source_artifact_id.as_deref() {
                let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
                    .await?
                    .filter(|artifact| artifact.task_id == task.id)
                    .ok_or_else(|| ServiceError::not_found("artifact", artifact_id.to_owned()))?;
                if artifact.kind != ArtifactKind::ReviewReport {
                    return Err(invalid_plan(
                        "a quality request source must be an exact ReviewReport Artifact",
                    ));
                }
                let digest = artifact.digest.clone().unwrap_or_else(|| {
                    sha256_hex(artifact.content.as_deref().unwrap_or_default().as_bytes())
                });
                ("review_report".to_owned(), artifact.id, digest)
            } else {
                (
                    "human".to_owned(),
                    user_id.to_owned(),
                    sha256_hex(input.summary.as_bytes()),
                )
            };
        let findings_json = serde_json::to_string(&input.findings)
            .map_err(|_| invalid_plan("quality findings cannot be serialized"))?;
        let request_digest = sha256_hex(
            canonical_json(&json!({
                "task_id": task.id,
                "source_kind": source_kind,
                "source_id": source_id,
                "category": input.category.to_string(),
                "target_kind": input.target_kind.to_string(),
                "target_task_role_id": input.target_task_role_id,
                "target_actor": input.target_actor,
                "summary": input.summary,
                "findings": input.findings,
                "resolution_condition": input.resolution_condition,
            }))?
            .as_bytes(),
        );
        if let Some(existing) = TaskCoordinationRepo::get_task_quality_request_by_key(
            &*self.db,
            &task.id,
            &input.idempotency_key,
        )
        .await?
        {
            if existing.request_digest != request_digest {
                return Err(coordination_error(
                    "coordination.idempotency_conflict",
                    "quality idempotency key already identifies different content",
                ));
            }
            return Ok(existing);
        }
        let target_role = match input.target_task_role_id.as_deref() {
            Some(role_id) => Some(
                TaskRoleRepo::get_by_id(&*self.db, role_id)
                    .await?
                    .filter(|role| role.task_id == task.id)
                    .ok_or_else(|| ServiceError::not_found("task_role", role_id.to_owned()))?,
            ),
            None => None,
        };
        let category_matches_target = matches!(
            (input.category, input.target_kind),
            (
                TaskQualityCategory::DeliveryCorrection,
                TaskQualityTargetKind::Implementation
            ) | (
                TaskQualityCategory::PlanRevision,
                TaskQualityTargetKind::Orchestrator
            ) | (
                TaskQualityCategory::Specialist,
                TaskQualityTargetKind::Specialist
            ) | (
                TaskQualityCategory::HumanDecision,
                TaskQualityTargetKind::Human
            )
        );
        if !category_matches_target {
            return Err(invalid_plan(
                "quality category and target kind do not describe a supported route",
            ));
        }
        if input.target_kind == TaskQualityTargetKind::Human && input.target_actor.is_none() {
            return Err(invalid_plan(
                "a human-targeted quality request requires an exact Actor",
            ));
        }
        if input.target_kind != TaskQualityTargetKind::Human
            && target_role.is_none()
            && input.target_actor.is_none()
        {
            return Err(invalid_plan(
                "a quality request must name an exact target TaskRole or Actor",
            ));
        }
        if let (Some(role), Some(actor)) = (target_role.as_ref(), input.target_actor.as_ref()) {
            let members = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
            if !members.iter().any(|member| {
                member.status == RoleMembershipStatus::Active && member.actor_ref() == *actor
            }) {
                return Err(coordination_error(
                    "coordination.not_authorized",
                    "the quality target Actor is not an active member of that TaskRole",
                ));
            }
        } else if let Some(role) = target_role.as_ref() {
            let members = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
            if !members
                .iter()
                .any(|member| member.status == RoleMembershipStatus::Active)
            {
                return Err(invalid_plan("the target TaskRole has no active member"));
            }
        }
        if !TaskCoordinationRepo::consume_quality_request_budget(&*self.db, &task.id).await? {
            return Err(coordination_error(
                "coordination.budget_exhausted",
                "the Task quality-request budget is exhausted",
            ));
        }
        let request_id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
            "task.quality_requested",
            "task_quality_request",
            &request_id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "quality_request_id": request_id,
                "category": input.category.to_string(),
                "target_kind": input.target_kind.to_string(),
                "source_kind": source_kind,
            }),
            &now,
        );
        let mut state = if input.category == TaskQualityCategory::HumanDecision {
            TaskQualityState::RequiresHuman
        } else {
            TaskQualityState::Requested
        };
        let mut handoff_id = None;
        if state != TaskQualityState::RequiresHuman {
            let target = match (input.target_actor.clone(), target_role.as_ref()) {
                (Some(actor), _) => db::CollaborationTarget::Actor(actor),
                (None, Some(role)) => db::CollaborationTarget::Role(role.id.clone()),
                _ => db::CollaborationTarget::Task,
            };
            let handoff = self
                .collaboration
                .create_handoff(
                    source.clone(),
                    CreateHandoffInput {
                        task_id: task.id.clone(),
                        source_role_id: None,
                        target,
                        work_unit_id: None,
                        intent: match input.category {
                            TaskQualityCategory::DeliveryCorrection => db::HandoffIntent::Rework,
                            TaskQualityCategory::PlanRevision => db::HandoffIntent::Delegation,
                            TaskQualityCategory::Specialist => db::HandoffIntent::Investigate,
                            TaskQualityCategory::HumanDecision => {
                                db::HandoffIntent::DecisionRequest
                            }
                        },
                        parent_execution_id: None,
                        expected_policy_ref: Some("forge.task-quality-request".to_owned()),
                        artifact_ids: input.source_artifact_id.iter().cloned().collect(),
                    },
                )
                .await?;
            handoff_id = Some(handoff.id);
            state = TaskQualityState::Notified;
        }
        let write = TaskCoordinationRepo::create_task_quality_request(
            &*self.db,
            db::CreateTaskQualityRequestInput {
                id: request_id,
                task_id: task.id,
                source_kind,
                source_id,
                category: input.category,
                target_kind: input.target_kind,
                target_task_role_id: input.target_task_role_id,
                target_actor: input.target_actor,
                summary: input.summary,
                findings_json,
                resolution_condition: input.resolution_condition,
                state,
                handoff_id,
                work_unit_id: None,
                source_digest,
                request_digest,
                idempotency_key: input.idempotency_key,
                created_by: actor,
                created_at: now,
                event,
            },
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok(write.request)
    }

    pub async fn list_quality_requests(
        &self,
        task_id: &str,
        user_id: &str,
    ) -> Result<Vec<TaskQualityRequest>> {
        self.collaboration
            .authorize_source(
                task_id,
                &CollaborationActorSource::Human(user_id.to_owned()),
            )
            .await?;
        Ok(TaskCoordinationRepo::list_task_quality_requests(&*self.db, task_id, 100).await?)
    }

    pub async fn resolve_quality_request(
        &self,
        task_id: &str,
        request_id: &str,
        user_id: &str,
        expected_version: i64,
        state: TaskQualityState,
    ) -> Result<TaskQualityRequest> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        let request = TaskCoordinationRepo::get_task_quality_request(&*self.db, request_id)
            .await?
            .filter(|request| request.task_id == task.id)
            .ok_or_else(|| {
                ServiceError::not_found("task_quality_request", request_id.to_owned())
            })?;
        let now = now_rfc3339();
        let event = self.event(
            "task.quality_request_resolved",
            "task_quality_request",
            &request.id,
            "human",
            Some(user_id),
            &task.id,
            json!({
                "task_id": task.id,
                "quality_request_id": request.id,
                "state": state.to_string(),
            }),
            &now,
        );
        Ok(TaskCoordinationRepo::update_task_quality_request_state(
            &*self.db,
            &request.id,
            expected_version,
            state,
            None,
            None,
            &now,
            Some(event),
        )
        .await?)
    }

    pub async fn create_intervention(
        &self,
        task_id: &str,
        user_id: &str,
        input: InterventionInput,
    ) -> Result<TaskIntervention> {
        self.create_intervention_as(
            task_id,
            CollaborationActorSource::Human(user_id.to_owned()),
            user_id,
            input,
        )
        .await
    }

    /// Project Copilot uses the same durable intervention circuit. It may only
    /// record an instruction or request a replan; it cannot pause, cancel,
    /// revoke, or decide for the Human.
    pub async fn create_intervention_as(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        authority_user_id: &str,
        input: InterventionInput,
    ) -> Result<TaskIntervention> {
        if matches!(source, CollaborationActorSource::ProjectCopilot(_))
            && !matches!(
                input.kind,
                TaskInterventionKind::Instruction | TaskInterventionKind::RequestReplan
            )
        {
            return Err(coordination_error(
                "coordination.not_authorized",
                "Project Copilot may only record an instruction or request a replan",
            ));
        }
        let user_id = authority_user_id;
        let (task, actor) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
            return Err(invalid_plan("intervention idempotency key is invalid"));
        }
        if input.kind == TaskInterventionKind::Decision
            && (input.plan_artifact_id.is_none() || input.approve.is_none())
        {
            return Err(invalid_plan(
                "a decision intervention requires an exact plan Artifact and decision",
            ));
        }
        let payload = json!({
            "kind": input.kind.to_string(),
            "instruction": input.instruction,
            "plan_artifact_id": input.plan_artifact_id,
            "decision": input.approve,
            "rationale": input.rationale,
            "reason": input.reason,
            "expected_lifecycle_version": input.expected_lifecycle_version,
            "brief": input.brief.as_ref().map(|brief| json!({
                "expected_task_version": brief.expected_task_version,
                "expected_coordination_version": brief.expected_coordination_version,
                "brief": brief.brief,
                "idempotency_key": brief.idempotency_key,
            })),
        });
        let payload_json = serde_json::to_string(&payload)
            .map_err(|_| invalid_plan("intervention payload cannot be serialized"))?;
        let request_digest = sha256_hex(payload_json.as_bytes());
        let intervention_id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
            "task.intervention_recorded",
            "task_intervention",
            &intervention_id,
            &actor.kind().to_string(),
            Some(actor.id()),
            &task.id,
            json!({
                "task_id": task.id,
                "intervention_id": intervention_id,
                "kind": input.kind.to_string(),
            }),
            &now,
        );
        let write = TaskCoordinationRepo::create_task_intervention(
            &*self.db,
            db::CreateTaskInterventionInput {
                id: intervention_id,
                task_id: task.id.clone(),
                kind: input.kind,
                source_kind: match &source {
                    CollaborationActorSource::Human(_) => "human".to_owned(),
                    CollaborationActorSource::ProjectCopilot(_) => "project_copilot".to_owned(),
                    CollaborationActorSource::Execution(_) => "human".to_owned(),
                },
                source_id: match &source {
                    CollaborationActorSource::Human(id) => id.clone(),
                    CollaborationActorSource::ProjectCopilot(id) => id.clone(),
                    CollaborationActorSource::Execution(id) => id.clone(),
                },
                payload_json,
                request_digest,
                idempotency_key: input.idempotency_key.clone(),
                expected_task_version: input.expected_task_version,
                expected_coordination_version: input.expected_coordination_version,
                created_at: now.clone(),
                event,
            },
        )
        .await?;
        if write.replayed {
            return Ok(write.intervention);
        }
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        match self
            .apply_intervention(&task, &source, user_id, &write.intervention, &input)
            .await
        {
            Ok(()) => {
                self.set_intervention_state(
                    &write.intervention,
                    TaskInterventionState::Applied,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await
            }
            Err(error) => {
                let (code, message) = match &error {
                    ServiceError::Coordination { code, message } => (*code, message.clone()),
                    other => ("coordination.intervention_failed", other.to_string()),
                };
                self.set_intervention_state(
                    &write.intervention,
                    TaskInterventionState::Rejected,
                    None,
                    None,
                    None,
                    Some(code),
                    Some(&message),
                )
                .await?;
                Err(error)
            }
        }
    }

    async fn apply_intervention(
        &self,
        task: &Task,
        source: &CollaborationActorSource,
        user_id: &str,
        intervention: &TaskIntervention,
        input: &InterventionInput,
    ) -> Result<()> {
        if let Some(expected_task_version) = input.expected_task_version {
            if task.version != expected_task_version {
                return Err(db::DbError::TaskVersionConflict {
                    expected: expected_task_version,
                    actual: task.version,
                }
                .into());
            }
        }
        match input.kind {
            TaskInterventionKind::Instruction => {
                let instruction = input
                    .instruction
                    .as_deref()
                    .filter(|value| bounded(value, MAX_INSTRUCTION_BYTES))
                    .ok_or_else(|| invalid_plan("instruction must contain 1 to 16384 bytes"))?;
                self.collaboration
                    .create_message(
                        source.clone(),
                        CreateMessageInput {
                            task_id: task.id.clone(),
                            target: db::CollaborationTarget::Task,
                            work_unit_id: None,
                            body: instruction.to_owned(),
                            artifact_ids: Vec::new(),
                        },
                    )
                    .await?;
                Ok(())
            }
            TaskInterventionKind::Pause
            | TaskInterventionKind::Resume
            | TaskInterventionKind::Cancel => {
                let expected_lifecycle_version =
                    input.expected_lifecycle_version.ok_or_else(|| {
                        invalid_plan("intervention requires expected_lifecycle_version")
                    })?;
                let lifecycle =
                    TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
                let requested_by = ApiActorRef::Human(user_id.to_owned());
                match input.kind {
                    TaskInterventionKind::Pause => {
                        lifecycle
                            .pause_task(
                                &task.id,
                                task.version,
                                expected_lifecycle_version,
                                &requested_by,
                                input.reason.clone(),
                                &format!("intervention:{}", intervention.id),
                            )
                            .await?;
                    }
                    TaskInterventionKind::Resume => {
                        lifecycle
                            .resume_task(
                                &task.id,
                                task.version,
                                expected_lifecycle_version,
                                &requested_by,
                                &format!("intervention:{}", intervention.id),
                            )
                            .await?;
                    }
                    TaskInterventionKind::Cancel => {
                        lifecycle
                            .transition_at_lifecycle_version(
                                crate::task_lifecycle::TransitionLifecycleInput {
                                    task_id: task.id.clone(),
                                    expected_task_version: task.version,
                                    to_state: TaskLifecycleState::Cancelled,
                                    cause: crate::task_lifecycle::LifecycleCause::Actor(
                                        api_types::Actor::User {
                                            user_id: Some(user_id.to_owned()),
                                            source: api_types::UserActionSource::Api,
                                        },
                                    ),
                                    reason_kind: Some("intervention".to_owned()),
                                    reason_ref: Some(intervention.id.clone()),
                                    idempotency_key: format!(
                                        "intervention-cancel:{}",
                                        intervention.id
                                    ),
                                },
                                expected_lifecycle_version,
                            )
                            .await?;
                        lifecycle
                            .record_cancelled_dispatch_after_cancel(
                                &task.id,
                                task.version,
                                expected_lifecycle_version,
                                &requested_by,
                                input.reason.clone(),
                                &format!("intervention:{}", intervention.id),
                            )
                            .await?;
                    }
                    _ => unreachable!(),
                }
                Ok(())
            }
            TaskInterventionKind::RevokeAuthorization => {
                let expected_coordination_version =
                    input.expected_coordination_version.ok_or_else(|| {
                        invalid_plan("revocation requires expected_coordination_version")
                    })?;
                self.revoke(
                    &task.id,
                    user_id,
                    task.version,
                    expected_coordination_version,
                    input.reason.clone(),
                    &format!("intervention:{}", intervention.id),
                )
                .await?;
                Ok(())
            }
            TaskInterventionKind::Decision => {
                let artifact_id = input
                    .plan_artifact_id
                    .as_deref()
                    .ok_or_else(|| invalid_plan("decision requires plan_artifact_id"))?;
                let expected_coordination_version =
                    input.expected_coordination_version.ok_or_else(|| {
                        invalid_plan("decision requires expected_coordination_version")
                    })?;
                let plan = TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id)
                    .await?
                    .filter(|plan| plan.task_id == task.id)
                    .ok_or_else(|| {
                        ServiceError::not_found("task_coordination_plan", artifact_id.to_owned())
                    })?;
                self.approve_plan(
                    &task.id,
                    artifact_id,
                    user_id,
                    PlanDecisionInput {
                        expected_coordination_version,
                        expected_plan_digest: plan.plan_digest,
                        approve: input.approve.unwrap_or(false),
                        rationale: input.rationale.clone(),
                        idempotency_key: format!("intervention:{}", intervention.id),
                    },
                )
                .await?;
                Ok(())
            }
            TaskInterventionKind::RequestReplan => {
                let brief = input.brief.clone().ok_or_else(|| {
                    invalid_plan("request_replan requires the new coordination brief")
                })?;
                if brief.expected_coordination_version
                    != input.expected_coordination_version.unwrap_or(0)
                {
                    return Err(coordination_error(
                        "coordination.version_conflict",
                        "request_replan coordination version is inconsistent",
                    ));
                }
                self.request_turn(&task.id, user_id, brief).await?;
                Ok(())
            }
        }
    }

    async fn set_intervention_state(
        &self,
        intervention: &TaskIntervention,
        state: TaskInterventionState,
        proposal_id: Option<Option<String>>,
        decision_id: Option<Option<String>>,
        coordination_request_id: Option<Option<String>>,
        failure_code: Option<&str>,
        failure_message: Option<&str>,
    ) -> Result<TaskIntervention> {
        let now = now_rfc3339();
        let event = self.event(
            "task.intervention_state_changed",
            "task_intervention",
            &intervention.id,
            "system",
            None,
            &intervention.task_id,
            json!({
                "task_id": intervention.task_id,
                "intervention_id": intervention.id,
                "state": state.to_string(),
                "failure_code": failure_code,
            }),
            &now,
        );
        Ok(TaskCoordinationRepo::update_task_intervention_state(
            &*self.db,
            &intervention.id,
            intervention.version,
            state,
            proposal_id,
            decision_id,
            coordination_request_id,
            Some(failure_code.map(str::to_owned)),
            Some(failure_message.map(str::to_owned)),
            &now,
            Some(event),
        )
        .await?)
    }

    pub async fn list_interventions(
        &self,
        task_id: &str,
        user_id: &str,
    ) -> Result<Vec<TaskIntervention>> {
        self.collaboration
            .authorize_source(
                task_id,
                &CollaborationActorSource::Human(user_id.to_owned()),
            )
            .await?;
        Ok(TaskCoordinationRepo::list_task_interventions(&*self.db, task_id, 100).await?)
    }

    async fn require_coordination(&self, task_id: &str) -> Result<TaskCoordination> {
        TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task coordination", task_id.to_owned()))
    }

    fn envelope(&self, coordination: &TaskCoordination) -> Result<CoordinationEnvelope> {
        serde_json::from_str(&coordination.envelope_json).map_err(|error| {
            invalid_plan(format!("stored coordination envelope is invalid: {error}"))
        })
    }

    fn parse_brief(&self, request: &TaskCoordinationRequest) -> Result<CoordinationBrief> {
        serde_json::from_str(&request.brief_json)
            .map_err(|error| invalid_plan(format!("stored coordination brief is invalid: {error}")))
    }

    fn parse_plan(&self, content: &str) -> Result<WorkUnitPlan> {
        let plan: WorkUnitPlan = serde_json::from_str(content).map_err(|error| {
            invalid_plan(format!("coordination plan is not valid v1 JSON: {error}"))
        })?;
        if plan.schema_version != 1 {
            return Err(unsupported_item(
                "only work_unit_plan schema version 1 is supported",
            ));
        }
        if plan.work_units.is_empty() || plan.work_units.len() > MAX_PLAN_WORK_UNITS {
            return Err(invalid_plan(
                "the plan must contain between 1 and 4 WorkUnits",
            ));
        }
        Ok(plan)
    }

    async fn current_team(&self, task_id: &str) -> Result<Vec<TeamRoleView>> {
        let roles = TaskRoleRepo::list_by_task(&*self.db, task_id).await?;
        let mut team = Vec::with_capacity(roles.len());
        for role in roles {
            let configuration =
                crate::role_capabilities::current_task_role_configuration(&self.db, &role.id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::invalid_operation(format!(
                            "TaskRole {} has no capability configuration",
                            role.id
                        ))
                    })?;
            let members = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false).await?;
            let mut member_views = Vec::new();
            for member in members
                .into_iter()
                .filter(|member| member.status == RoleMembershipStatus::Active)
            {
                let (display_name, harness) = self.actor_display(&member.actor_ref()).await?;
                member_views.push(TeamMemberView {
                    id: member.id.clone(),
                    actor_ref: member.actor_ref(),
                    display_name,
                    harness,
                    status: member.status.to_string(),
                    version: member.version,
                });
            }
            team.push(TeamRoleView {
                id: role.id.clone(),
                role: role.role.clone(),
                display_label: configuration.display_label.clone(),
                coordination_mode: role.coordination_mode,
                capabilities: configuration.capabilities.clone(),
                configuration,
                version: role.version,
                members: member_views,
            });
        }
        Ok(team)
    }

    fn team_snapshot_json(&self, team: &[TeamRoleView]) -> Result<String> {
        let roles = team
            .iter()
            .map(|role| {
                json!({
                    "id": role.id,
                    "role": role.role,
                    "version": role.version,
                    "coordination_mode": role.coordination_mode.map(|mode| mode.to_string()),
                    "capabilities": role.capabilities,
                    "configuration": role.configuration,
                })
            })
            .collect::<Vec<_>>();
        let memberships = team
            .iter()
            .flat_map(|role| {
                role.members.iter().map(|member| {
                    json!({
                        "id": member.id,
                        "task_role_id": role.id,
                        "actor": member.actor_ref,
                        "status": member.status,
                        "version": member.version,
                    })
                })
            })
            .collect::<Vec<_>>();
        canonical_json(&json!({
            "schema_version": 1,
            "task_roles": roles,
            "memberships": memberships,
        }))
    }

    fn team_digest(&self, team: &[TeamRoleView]) -> Result<String> {
        Ok(sha256_hex(self.team_snapshot_json(team)?.as_bytes()))
    }

    async fn actor_display(&self, actor: &ActorRef) -> Result<(String, Option<String>)> {
        match actor {
            ActorRef::Human(id) => {
                let user = UserRepo::get_user_by_id(&*self.db, id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("user", id.clone()))?;
                Ok((user.display_name.unwrap_or(user.email), None))
            }
            ActorRef::Agent(id) => {
                let agent = AgentRepo::get_by_id(&*self.db, id)
                    .await?
                    .ok_or_else(|| ServiceError::not_found("agent", id.clone()))?;
                Ok((agent.name, Some(agent.executor_type)))
            }
        }
    }

    async fn require_actor_exists(&self, actor: &ActorRef) -> Result<()> {
        match actor {
            ActorRef::Human(id) => {
                if UserRepo::get_user_by_id(&*self.db, id).await?.is_none() {
                    return Err(ServiceError::not_found("user", id.clone()));
                }
            }
            ActorRef::Agent(id) => {
                if AgentRepo::get_by_id(&*self.db, id).await?.is_none() {
                    return Err(ServiceError::not_found("agent", id.clone()));
                }
            }
        }
        Ok(())
    }

    async fn require_active_member(&self, role: &TeamRoleView, actor: &ActorRef) -> Result<()> {
        if !role.members.iter().any(|member| {
            member.actor_ref.kind() == actor.kind() && member.actor_ref.id() == actor.id()
        }) {
            return Err(coordination_error(
                "coordination.not_authorized",
                "the selected Actor is not an active member of that TaskRole",
            ));
        }
        Ok(())
    }

    async fn resolve_coordinator(
        &self,
        coordination: &TaskCoordination,
        team: &[TeamRoleView],
    ) -> Result<Option<CoordinatorView>> {
        let (Some(role_id), Some(actor)) = (
            coordination.coordinator_task_role_id.as_deref(),
            coordination.coordinator_actor.as_ref(),
        ) else {
            return Ok(None);
        };
        let Some(role) = team.iter().find(|role| role.id == role_id) else {
            return Ok(None);
        };
        if !role.members.iter().any(|member| member.actor_ref == *actor) {
            return Ok(None);
        }
        let (display_name, harness) = self.actor_display(actor).await?;
        Ok(Some(CoordinatorView {
            task_role_id: role.id.clone(),
            role: role.role.clone(),
            actor_ref: actor.clone(),
            display_name,
            harness,
        }))
    }

    async fn coordination_blockers(
        &self,
        _task: &Task,
        coordination: &TaskCoordination,
        team: &[TeamRoleView],
        coordinator: Option<&CoordinatorView>,
        team_digest: &str,
    ) -> Result<Vec<CoordinationBlocker>> {
        let mut blockers = Vec::new();
        if coordination.revoked_at.is_some() {
            blockers.push(CoordinationBlocker {
                code: "coordination.authorization_revoked".to_owned(),
                message: "Task coordination authorization is revoked".to_owned(),
                task_role_id: None,
                actor_ref: None,
            });
        }
        if coordination.mode == TaskCoordinationMode::Manual {
            return Ok(blockers);
        }
        if coordination.coordinator_task_role_id.is_none()
            || coordination.coordinator_actor.is_none()
        {
            blockers.push(CoordinationBlocker {
                code: "coordination.coordinator_missing".to_owned(),
                message: "select an Agent coordinator with orchestration capability".to_owned(),
                task_role_id: None,
                actor_ref: None,
            });
            return Ok(blockers);
        }
        if let Some(role) = team
            .iter()
            .find(|role| Some(&role.id) == coordination.coordinator_task_role_id.as_ref())
        {
            if !role
                .capabilities
                .contains(&TaskRoleCapability::Orchestration)
            {
                blockers.push(CoordinationBlocker {
                    code: "coordination.coordinator_capability_missing".to_owned(),
                    message: "the coordinator TaskRole no longer has orchestration capability"
                        .to_owned(),
                    task_role_id: Some(role.id.clone()),
                    actor_ref: coordination.coordinator_actor.clone(),
                });
            }
        }
        if coordinator.is_none() {
            blockers.push(CoordinationBlocker {
                code: "coordination.coordinator_not_member".to_owned(),
                message: "the coordinator is no longer an active TaskRole member".to_owned(),
                task_role_id: coordination.coordinator_task_role_id.clone(),
                actor_ref: coordination.coordinator_actor.clone(),
            });
        }
        if team_digest != coordination.team_digest {
            blockers.push(CoordinationBlocker {
                code: "coordination.authorization_stale".to_owned(),
                message: "the Task team changed; reauthorize coordination".to_owned(),
                task_role_id: None,
                actor_ref: None,
            });
        }
        if coordination.turn_count >= coordination.max_turns {
            blockers.push(CoordinationBlocker {
                code: "coordination.budget_exhausted".to_owned(),
                message: "the Task coordination turn budget is exhausted".to_owned(),
                task_role_id: None,
                actor_ref: None,
            });
        }
        Ok(blockers)
    }

    async fn validate_plan(
        &self,
        task: &Task,
        coordination: &TaskCoordination,
        envelope: &CoordinationEnvelope,
        team: &[TeamRoleView],
        current_team_digest: &str,
        request: &TaskCoordinationRequest,
        plan: &WorkUnitPlan,
        materializing: bool,
    ) -> Result<()> {
        if plan.task_id != task.id
            || plan.request_id != request.id
            || plan.authorization_version != coordination.authorization_version
            || plan.team_digest != *current_team_digest
            || plan.envelope_digest != coordination.envelope_digest
            || plan.objective_digest != request.objective_digest
        {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "the plan does not bind the current Task, request, authorization, team, and envelope",
            ));
        }
        if materializing && plan.task_version != task.version {
            return Err(db::DbError::TaskVersionConflict {
                expected: plan.task_version,
                actual: task.version,
            }
            .into());
        }
        if plan.work_units.len() > envelope.max_work_units {
            return Err(unsupported_item(
                "the plan exceeds the authorized WorkUnit limit",
            ));
        }
        let mut keys = HashSet::new();
        for unit in &plan.work_units {
            if !bounded(&unit.key, 128) || !keys.insert(unit.key.clone()) {
                return Err(invalid_plan("WorkUnit keys must be unique and bounded"));
            }
            if !bounded(&unit.title, MAX_TITLE_BYTES) || !bounded(&unit.scope, MAX_SCOPE_BYTES) {
                return Err(invalid_plan(
                    "WorkUnit title or scope is empty or too large",
                ));
            }
            if unit
                .execution_instructions
                .as_deref()
                .is_some_and(|value| value.len() > MAX_INSTRUCTIONS_BYTES)
            {
                return Err(invalid_plan("WorkUnit instructions exceed 64 KiB"));
            }
            if unit.acceptance.len() > MAX_ACCEPTANCE_ITEMS
                || unit.depends_on.len() > MAX_DEPENDENCIES
            {
                return Err(invalid_plan(
                    "WorkUnit acceptance or dependency list exceeds the v1 limit",
                ));
            }
        }
        for unit in &plan.work_units {
            let role_name = db::canonical_task_role_name(&unit.role)
                .ok_or_else(|| invalid_plan("WorkUnit role is not a TaskRole name"))?;
            let role = team
                .iter()
                .find(|role| role.role == role_name)
                .ok_or_else(|| invalid_plan("WorkUnit role does not exist on this Task"))?;
            if unit.requires_integration
                && (!envelope.allow_integration || !envelope.allow_repository_write)
            {
                return Err(unsupported_item(
                    "the plan requests repository integration outside the envelope",
                ));
            }
            if unit.requires_integration && task.repo_id.is_none() {
                return Err(invalid_plan(
                    "a repository WorkUnit requires a repository-bound Task",
                ));
            }
            if let Some(actor) = unit.assigned_actor.as_ref() {
                self.require_active_member(role, &actor.clone().into_domain())
                    .await?;
                self.require_actor_exists(&actor.clone().into_domain())
                    .await?;
            }
            if let Some(review_key) = unit.review_subject_key.as_deref() {
                if !keys.contains(review_key) || review_key == unit.key {
                    return Err(invalid_plan(
                        "a review subject must name a different WorkUnit in this plan",
                    ));
                }
                if role.role != "reviewer" {
                    return Err(invalid_plan(
                        "a formal review WorkUnit must use the TaskRole named reviewer",
                    ));
                }
                if !role
                    .capabilities
                    .contains(&TaskRoleCapability::FormalReview)
                {
                    return Err(invalid_plan(
                        "the reviewer TaskRole lacks formal_review capability",
                    ));
                }
                if !envelope
                    .allowed_review_roles
                    .iter()
                    .any(|allowed| allowed == "reviewer")
                {
                    return Err(unsupported_item(
                        "the envelope does not allow the reviewer TaskRole",
                    ));
                }
                let reviewed = plan
                    .work_units
                    .iter()
                    .find(|candidate| candidate.key == review_key)
                    .ok_or_else(|| invalid_plan("review subject key is missing"))?;
                if unit.assigned_actor.is_some()
                    && unit.assigned_actor == reviewed.assigned_actor
                    && envelope.require_independent_review
                {
                    return Err(invalid_plan(
                        "review must be performed by an independent Actor",
                    ));
                }
                if reviewed.review_subject_key.is_some() {
                    return Err(invalid_plan("review WorkUnits cannot review each other"));
                }
            } else if role.role == "reviewer"
                && role
                    .capabilities
                    .contains(&TaskRoleCapability::FormalReview)
            {
                return Err(invalid_plan(
                    "a reviewer WorkUnit requires an explicit review_subject_key",
                ));
            }
            for dependency in &unit.depends_on {
                if dependency == &unit.key || !keys.contains(dependency) {
                    return Err(invalid_plan(
                        "WorkUnit dependencies must name another key in this plan",
                    ));
                }
            }
            if let Some(parent) = unit.parent_key.as_deref() {
                if parent == unit.key || !keys.contains(parent) {
                    return Err(invalid_plan(
                        "WorkUnit parent_key must name another key in this plan",
                    ));
                }
            }
            for artifact_id in &unit.artifact_inputs {
                let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
                    .await?
                    .filter(|artifact| artifact.task_id == task.id)
                    .ok_or_else(|| invalid_plan("WorkUnit Artifact input is not in this Task"))?;
                if artifact.digest.as_deref().is_none_or(str::is_empty)
                    || artifact.execution_producer().is_none()
                {
                    return Err(invalid_plan(
                        "WorkUnit Artifact inputs require an exact completed producer and digest",
                    ));
                }
            }
        }
        let by_key = plan
            .work_units
            .iter()
            .map(|unit| (unit.key.as_str(), unit))
            .collect::<BTreeMap<_, _>>();
        for unit in &plan.work_units {
            let mut seen = HashSet::new();
            let mut stack = unit.depends_on.clone();
            while let Some(key) = stack.pop() {
                if !seen.insert(key.clone()) || key == unit.key {
                    return Err(invalid_plan("the plan dependency graph contains a cycle"));
                }
                if let Some(next) = by_key.get(key.as_str()) {
                    stack.extend(next.depends_on.iter().cloned());
                }
            }
            let mut parent = unit.parent_key.as_deref();
            let mut depth = 0;
            while let Some(key) = parent {
                depth += 1;
                if depth > MAX_PLAN_WORK_UNITS || key == unit.key {
                    return Err(invalid_plan("the plan parent hierarchy is cyclic"));
                }
                parent = by_key.get(key).and_then(|next| next.parent_key.as_deref());
            }
        }
        Ok(())
    }

    fn is_within_envelope(&self, envelope: &CoordinationEnvelope, plan: &WorkUnitPlan) -> bool {
        plan.work_units.len() <= envelope.max_work_units
            && plan.work_units.iter().all(|unit| {
                (!unit.requires_integration
                    || (envelope.allow_integration && envelope.allow_repository_write))
                    && unit.review_subject_key.as_deref().is_none_or(|_| {
                        envelope
                            .allowed_review_roles
                            .iter()
                            .any(|role| role == "reviewer")
                    })
            })
    }

    async fn build_materialized_work_units(
        &self,
        task: &Task,
        plan: &WorkUnitPlan,
        _team: &[TeamRoleView],
    ) -> Result<Vec<db::MaterializedWorkUnit>> {
        let mut ids = HashMap::new();
        for unit in &plan.work_units {
            ids.insert(
                unit.key.clone(),
                deterministic_work_unit_id(&plan.request_id, &unit.key)?,
            );
        }
        let mut ordered: Vec<&PlanWorkUnit> = Vec::with_capacity(plan.work_units.len());
        let mut inserted = HashSet::new();
        let mut remaining: Vec<&PlanWorkUnit> = plan.work_units.iter().collect();
        while !remaining.is_empty() {
            let mut progress = false;
            remaining.retain(|unit| {
                if unit
                    .parent_key
                    .as_deref()
                    .is_none_or(|parent| inserted.contains(parent))
                {
                    inserted.insert(unit.key.clone());
                    ordered.push(unit);
                    progress = true;
                    false
                } else {
                    true
                }
            });
            if !progress {
                return Err(invalid_plan("the plan parent hierarchy cannot be ordered"));
            }
        }
        let mut result = Vec::with_capacity(plan.work_units.len());
        for unit in ordered {
            let id = ids
                .get(&unit.key)
                .cloned()
                .ok_or_else(|| invalid_plan("WorkUnit key mapping is missing"))?;
            let role_name = db::canonical_task_role_name(&unit.role)
                .ok_or_else(|| invalid_plan("WorkUnit role is invalid"))?;
            let mut artifact_inputs = Vec::new();
            for artifact_id in &unit.artifact_inputs {
                let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
                    .await?
                    .filter(|artifact| artifact.task_id == task.id)
                    .ok_or_else(|| invalid_plan("WorkUnit Artifact input is missing"))?;
                let (producer_execution_id, _) =
                    artifact.execution_producer().ok_or_else(|| {
                        invalid_plan("WorkUnit Artifact input requires an Execution producer")
                    })?;
                let producer_execution_id = producer_execution_id.to_owned();
                let artifact_id = artifact.id.clone();
                let digest = artifact
                    .digest
                    .clone()
                    .ok_or_else(|| invalid_plan("WorkUnit Artifact input requires a digest"))?;
                artifact_inputs.push(db::MaterializedArtifactInput {
                    artifact_id,
                    digest,
                    producer_execution_id,
                });
            }
            let review_subject_work_unit_id = unit
                .review_subject_key
                .as_ref()
                .and_then(|key| ids.get(key).cloned());
            result.push(db::MaterializedWorkUnit {
                id,
                key: unit.key.clone(),
                title: unit.title.clone(),
                scope: unit.scope.clone(),
                execution_instructions: unit.execution_instructions.clone(),
                role: role_name,
                assigned_actor: unit.assigned_actor.clone().map(|actor| actor.into_domain()),
                parent_work_unit_id: unit
                    .parent_key
                    .as_ref()
                    .and_then(|key| ids.get(key).cloned()),
                depends_on_work_unit_ids: unit
                    .depends_on
                    .iter()
                    .filter_map(|key| ids.get(key).cloned())
                    .collect(),
                requires_integration: unit.requires_integration,
                artifact_inputs,
                review_subject_work_unit_id,
            });
        }
        Ok(result)
    }

    async fn plan_view(&self, plan: TaskCoordinationPlan) -> Result<PlanView> {
        let artifact = CollaborationRepo::get_artifact(&*self.db, &plan.artifact_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("artifact", plan.artifact_id.clone()))?;
        let content = artifact
            .content
            .as_deref()
            .ok_or_else(|| invalid_plan("plan Artifact is not inline JSON"))?;
        let parsed = self.parse_plan(content)?;
        let request =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &plan.request_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("task_coordination_request", plan.request_id.clone())
                })?;
        let materialization = TaskCoordinationRepo::get_task_coordination_materialization(
            &*self.db,
            &plan.artifact_id,
        )
        .await?;
        Ok(PlanView {
            plan,
            parsed,
            mode: request.mode,
            materialization,
        })
    }

    fn turn_request_digest(
        &self,
        task: &Task,
        coordination: &TaskCoordination,
        brief: &CoordinationBrief,
    ) -> Result<String> {
        Ok(sha256_hex(
            canonical_json(&json!({
                "operation": "request_turn",
                "task_id": task.id,
                "task_version": task.version,
                "authorization_version": coordination.authorization_version,
                "mode": coordination.mode.to_string(),
                "brief": brief,
                "team_digest": coordination.team_digest,
                "envelope_digest": coordination.envelope_digest,
            }))?
            .as_bytes(),
        ))
    }

    fn event(
        &self,
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        actor_type: &str,
        actor_id: Option<&str>,
        task_id: &str,
        payload: Value,
        now: &str,
    ) -> CreateDomainEvent {
        self.event_with_correlation(
            event_type,
            entity_type,
            entity_id,
            actor_type,
            actor_id,
            task_id,
            payload,
            now,
            &new_uuid_v4(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn event_with_correlation(
        &self,
        event_type: &str,
        entity_type: &str,
        entity_id: &str,
        actor_type: &str,
        actor_id: Option<&str>,
        task_id: &str,
        payload: Value,
        now: &str,
        correlation_id: &str,
    ) -> CreateDomainEvent {
        CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: event_type.to_owned(),
            entity_type: entity_type.to_owned(),
            entity_id: entity_id.to_owned(),
            actor_type: actor_type.to_owned(),
            actor_id: actor_id.map(str::to_owned),
            scope_type: "task".to_owned(),
            scope_id: task_id.to_owned(),
            correlation_id: correlation_id.to_owned(),
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("{event_type}:{entity_id}")),
            payload_json: payload.to_string(),
            created_at: now.to_owned(),
        }
    }
}

trait DomainActorExt {
    fn into_domain(self) -> ActorRef;
}

impl DomainActorExt for ApiActorRef {
    fn into_domain(self) -> ActorRef {
        match self {
            ApiActorRef::Human(id) => ActorRef::Human(id),
            ApiActorRef::Agent(id) => ActorRef::Agent(id),
        }
    }
}

fn deterministic_work_unit_id(request_id: &str, key: &str) -> Result<String> {
    let digest = Sha256::digest(
        [
            b"forge.task-coordination.work-unit.v1".as_slice(),
            request_id.as_bytes(),
            key.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_envelope_is_structurally_safe() {
        let envelope = CoordinationEnvelope::default();
        envelope.validate().expect("default envelope");
        assert_eq!(envelope.max_work_units, 4);
        assert!(!envelope.allow_new_task_roles);
        assert!(!envelope.allow_merge);
    }

    #[test]
    fn envelope_rejects_authority_grants() {
        for update in [
            |envelope: &mut CoordinationEnvelope| envelope.allow_new_task_roles = true,
            |envelope: &mut CoordinationEnvelope| envelope.allow_membership_changes = true,
            |envelope: &mut CoordinationEnvelope| envelope.allow_capability_changes = true,
        ] {
            let mut envelope = CoordinationEnvelope::default();
            update(&mut envelope);
            assert!(envelope.validate().is_err());
        }
    }

    #[test]
    fn deterministic_work_unit_ids_replay_exactly() {
        let first = deterministic_work_unit_id("request-1", "api").expect("id");
        let replay = deterministic_work_unit_id("request-1", "api").expect("id");
        let other_key = deterministic_work_unit_id("request-1", "ui").expect("id");
        let other_request = deterministic_work_unit_id("request-2", "api").expect("id");
        assert_eq!(first, replay);
        assert_ne!(first, other_key);
        assert_ne!(first, other_request);
    }

    #[test]
    fn coordination_brief_requires_an_objective() {
        assert!(CoordinationBrief::default().validate().is_err());
        CoordinationBrief {
            objective: "Deliver the read-only planning package".to_owned(),
            ..CoordinationBrief::default()
        }
        .validate()
        .expect("bounded objective");
    }
}
