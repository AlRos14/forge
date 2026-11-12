use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

use api_types::{ActorRef as ApiActorRef, TaskRoleCapability, TaskRoleConfiguration};
use db::{
    new_uuid_v4, now_rfc3339, ActorRef, AgentRepo, ArtifactKind, CollaborationRepo,
    CreateDomainEvent, ProjectPlanBatchAuthorizationBinding, ProjectPlanBatchRequestBinding,
    ProjectPlanBatchRequestOrigin, RoleMembershipRepo, RoleMembershipStatus, SqliteDb, Task,
    TaskCoordination, TaskCoordinationMaterialization, TaskCoordinationMode, TaskCoordinationPlan,
    TaskCoordinationPlanState, TaskCoordinationRepo, TaskCoordinationRequest,
    TaskCoordinationRequestState, TaskCoordinationWrite, TaskIntervention, TaskInterventionKind,
    TaskInterventionState, TaskLifecycleRepo, TaskLifecycleState, TaskQualityCategory,
    TaskQualityRequest, TaskQualityState, TaskQualityTargetKind, TaskRepo, TaskRoleRepo, UserRepo,
};
use events::EventBus;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::Row;

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
const MAX_PLAN_WORK_UNITS: usize = 64;
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
    /// Missing on historical envelopes means the previously authorized direct route.
    #[serde(default)]
    pub orchestrator_reviews_findings: bool,
}

impl Default for CoordinationEnvelope {
    fn default() -> Self {
        Self {
            schema_version: 1,
            max_work_units: 16,
            allow_new_task_roles: false,
            allow_membership_changes: false,
            allow_capability_changes: false,
            allow_repository_write: true,
            allow_external_actions: false,
            allow_integration: true,
            allow_merge: false,
            allowed_review_roles: vec!["reviewer".to_owned()],
            require_independent_review: true,
            orchestrator_reviews_findings: true,
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

pub(crate) fn semantic_unconfigured_manual_default(
    mode: Option<&str>,
    requested_by: Option<&str>,
    revoked_at: Option<&str>,
    envelope_text: Option<&str>,
) -> bool {
    if mode != Some("manual") || requested_by.is_some() || revoked_at.is_some() {
        return false;
    }
    let Some(envelope_text) = envelope_text else {
        return false;
    };
    let Ok(envelope) = serde_json::from_str::<CoordinationEnvelope>(envelope_text) else {
        return false;
    };
    let current_default = CoordinationEnvelope::default();
    let mut legacy_default = current_default.clone();
    legacy_default.max_work_units = 4;
    legacy_default.orchestrator_reviews_findings = false;
    envelope == current_default || envelope == legacy_default
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
    pub project_plan_batch_scope: Option<api_types::TaskCoordinationProjectBatchScopeResponse>,
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
    work_unit_service: Option<Arc<crate::WorkUnitService>>,
}

impl TaskCoordinationService {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<EventBus>) -> Self {
        Self {
            collaboration: CollaborationService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            domain_events: DomainEventService::new(Arc::clone(&db), Arc::clone(&event_bus)),
            db,
            event_bus,
            work_unit_service: None,
        }
    }

    pub fn with_work_unit_service(mut self, service: Arc<crate::WorkUnitService>) -> Self {
        self.work_unit_service = Some(service);
        self
    }

    pub async fn overview(&self, task_id: &str, user_id: &str) -> Result<Overview> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_source(task_id, &source)
            .await?;
        let coordination = self.require_coordination(&task.id).await?;
        let batch_scope_rows = sqlx::query(
            "SELECT b.id AS batch_id,b.state AS batch_state,bt.state AS task_state,bt.blocker
             FROM project_plan_batch_coordination_authorization a
             JOIN project_plan_batch b
               ON b.id=a.batch_id AND b.project_id=a.project_id
             JOIN project_plan_batch_task bt
               ON bt.batch_id=a.batch_id AND bt.task_id=a.task_id
             WHERE a.project_id=? AND a.task_id=? AND a.authorization_version=?
             ORDER BY b.id",
        )
        .bind(&task.project_id)
        .bind(&task.id)
        .bind(coordination.authorization_version)
        .fetch_all(self.db.pool())
        .await?;
        if batch_scope_rows.len() > 1 {
            return Err(ServiceError::Conflict(
                "Task coordination authorization has multiple Project batch origins".into(),
            ));
        }
        let project_plan_batch_scope = batch_scope_rows
            .into_iter()
            .next()
            .map(|row| -> std::result::Result<_, sqlx::Error> {
                Ok(api_types::TaskCoordinationProjectBatchScopeResponse {
                    batch_id: row.try_get("batch_id")?,
                    batch_state: row.try_get("batch_state")?,
                    task_state: row.try_get("task_state")?,
                    blocker: row.try_get("blocker")?,
                })
            })
            .transpose()?;
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
        let can_request = can_request && project_plan_batch_scope.is_none();
        Ok(Overview {
            task,
            coordination,
            envelope,
            team,
            coordinator,
            blockers,
            can_authorize: mutable,
            can_request,
            project_plan_batch_scope,
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
        self.authorize_inner(task_id, user_id, input, None, None)
            .await
    }

    pub async fn authorize_with_expected_team_generation(
        &self,
        task_id: &str,
        user_id: &str,
        input: AuthorizationInput,
        expected_team_generation: i64,
    ) -> Result<Overview> {
        self.authorize_inner(
            task_id,
            user_id,
            input,
            Some(expected_team_generation),
            None,
        )
        .await
    }

    /// Mints a TaskCoordination authorization that is durably scoped to one
    /// current Project batch. The exact grant receipt is written in the same
    /// database transaction as the authorization version change.
    pub async fn authorize_for_project_plan_batch(
        &self,
        task_id: &str,
        user_id: &str,
        input: AuthorizationInput,
        binding: ProjectPlanBatchAuthorizationBinding,
    ) -> Result<Overview> {
        if binding.authorized_by != user_id {
            return Err(ServiceError::AuthorizationDenied {
                message: "Project batch authorization must be issued by its current human approver"
                    .into(),
            });
        }
        self.authorize_inner(
            task_id,
            user_id,
            input,
            Some(binding.expected_team_generation),
            Some(binding),
        )
        .await
    }

    async fn authorize_inner(
        &self,
        task_id: &str,
        user_id: &str,
        input: AuthorizationInput,
        expected_team_generation: Option<i64>,
        batch_binding: Option<ProjectPlanBatchAuthorizationBinding>,
    ) -> Result<Overview> {
        let source = CollaborationActorSource::Human(user_id.to_owned());
        let (task, _) = self
            .collaboration
            .authorize_mutation_source(task_id, &source)
            .await?;
        if input.idempotency_key.trim().is_empty() || input.idempotency_key.len() > 256 {
            return Err(invalid_plan("idempotency key must contain 1 to 256 bytes"));
        }
        let mut envelope = match input.envelope.as_ref() {
            Some(value) => {
                serde_json::from_value::<CoordinationEnvelope>(value.clone()).map_err(|error| {
                    unsupported_item(format!("invalid coordination envelope: {error}"))
                })?
            }
            None => CoordinationEnvelope::default(),
        };
        if input.envelope.is_some() {
            envelope.validate()?;
        }
        if input.mode != TaskCoordinationMode::Manual
            && (input.coordinator_task_role_id.is_none() || input.coordinator_actor.is_none())
        {
            return Err(invalid_plan(
                "proposal and prepare_and_execute require an exact coordinator TaskRole and Actor",
            ));
        }
        let coordination_before = self.require_coordination(&task.id).await?;
        let current_mode = coordination_before.mode.to_string();
        let unconfigured_default_refresh = batch_binding.is_some()
            && input.envelope.is_none()
            && semantic_unconfigured_manual_default(
                Some(&current_mode),
                coordination_before.requested_by.as_deref(),
                coordination_before.revoked_at.as_deref(),
                Some(&coordination_before.envelope_json),
            );
        if expected_team_generation
            .is_some_and(|expected| coordination_before.team_generation != expected)
        {
            return Err(db::DbError::VersionConflict.into());
        }
        let team = self.current_team(&task.id).await?;
        if input.envelope.is_none() {
            envelope.allowed_review_roles = team
                .iter()
                .filter(|role| {
                    role.capabilities
                        .contains(&TaskRoleCapability::FormalReview)
                })
                .map(|role| role.role.clone())
                .collect();
            envelope.validate()?;
        }
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
                "unconfigured_default_refresh": unconfigured_default_refresh,
                "requested_by": user_id,
                "batch_scope": batch_binding.as_ref().map(|binding| (
                    binding.project_id.as_str(),
                    binding.batch_id.as_str(),
                    binding.expected_batch_version,
                    binding.expected_graph_version,
                    binding.expected_team_generation,
                )),
            }))?
            .as_bytes(),
        );
        let now = now_rfc3339();
        let mut event = self.event(
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
        event.dedupe_key = Some(format!(
            "task.coordination_authorized:{}:{}",
            task.id,
            input.expected_coordination_version + 1
        ));
        let write_input = db::AuthorizeTaskCoordinationInput {
            task_id: task.id.clone(),
            expected_coordination_version: input.expected_coordination_version,
            expected_team_generation: coordination_before.team_generation,
            mode: input.mode,
            coordinator_task_role_id: input.coordinator_task_role_id,
            coordinator_actor: coordinator,
            team_snapshot_json,
            team_digest,
            envelope_json,
            envelope_digest,
            unconfigured_default_refresh,
            requested_by: user_id.to_owned(),
            idempotency_key: input.idempotency_key,
            request_digest,
            receipt_id: new_uuid_v4(),
            created_at: now,
            event,
        };
        let write = if let Some(binding) = batch_binding {
            TaskCoordinationRepo::authorize_project_plan_batch_task_coordination(
                &*self.db,
                write_input,
                input.expected_task_version,
                binding,
            )
            .await?
        } else {
            TaskCoordinationRepo::authorize_task_coordination(
                &*self.db,
                write_input,
                input.expected_task_version,
            )
            .await?
        };
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
        let mut event = self.event(
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
        event.dedupe_key = Some(format!(
            "task.coordination_revoked:{}:{}",
            task.id,
            expected_coordination_version + 1
        ));
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
        self.request_turn_inner(task_id, user_id, input, None).await
    }

    /// Requests a continuation that is attached to one exact active Project
    /// batch origin. The repository atomically validates the origin, charges
    /// the turn budget, creates the request, links it to the batch Task, and
    /// appends the event.
    pub async fn request_project_batch_turn(
        &self,
        task_id: &str,
        user_id: &str,
        input: TurnRequestInput,
        binding: ProjectPlanBatchRequestBinding,
    ) -> Result<(TaskCoordinationRequest, bool)> {
        self.request_turn_inner(task_id, user_id, input, Some(binding))
            .await
    }

    async fn request_turn_inner(
        &self,
        task_id: &str,
        user_id: &str,
        input: TurnRequestInput,
        batch_binding: Option<ProjectPlanBatchRequestBinding>,
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
            let request_digest = self.turn_request_digest(&task, &coordination, &input.brief)?;
            if request.request_digest != request_digest {
                return Err(coordination_error(
                    "coordination.idempotency_conflict",
                    "idempotency key already identifies a different coordination request",
                ));
            }
            if batch_binding.is_none()
                && TaskCoordinationRepo::is_project_plan_batch_coordination_request(
                    &*self.db,
                    &request.id,
                )
                .await?
            {
                return Err(db::DbError::ProjectPlanBatchInactive.into());
            }
            if batch_binding.is_none() {
                return Ok((request, false));
            }
        }
        let team = self.current_team(&task.id).await?;
        if coordination.team_stale || self.team_digest(&team)? != coordination.team_digest {
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
        if let Some(binding) = batch_binding.as_ref() {
            if binding.authorized_by != user_id
                || binding.expected_task_version != input.expected_task_version
                || binding.expected_coordination_version != input.expected_coordination_version
            {
                return Err(db::DbError::ProjectPlanBatchInactive.into());
            }
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
        let create_input = db::CreateTaskCoordinationRequestInput {
            id: request_id,
            task_id: task.id.clone(),
            requested_by: user_id.to_owned(),
            expected_task_version: input.expected_task_version,
            authorization_version: coordination.authorization_version,
            mode: coordination.mode,
            brief_json,
            objective_digest,
            request_digest,
            idempotency_key: input.idempotency_key,
            created_at: now,
            event,
        };
        let write = if let Some(binding) = batch_binding {
            TaskCoordinationRepo::create_project_plan_batch_coordination_request(
                &*self.db,
                create_input,
                binding,
            )
            .await
        } else {
            TaskCoordinationRepo::create_task_coordination_request(&*self.db, create_input).await
        };
        let (request, committed) = match write {
            Ok(write) => write,
            Err(db::DbError::CoordinationTurnBudgetExhausted) => {
                return Err(coordination_error(
                    "coordination.budget_exhausted",
                    "the Task coordination turn budget is exhausted",
                ));
            }
            Err(error) => return Err(error.into()),
        };
        if let Some(event) = committed.as_ref() {
            self.domain_events.publish_committed(event);
        }
        Ok((request, committed.is_some()))
    }

    /// Resolves the exact producer Execution for a quality source Artifact into
    /// the still-active Project batch scope that admitted it. No latest
    /// Execution or Task-only inference is used.
    pub async fn project_batch_binding_for_quality_source(
        &self,
        task_id: &str,
        user_id: &str,
        source_artifact_id: &str,
        expected_task_version: i64,
        expected_coordination_version: i64,
    ) -> Result<Option<ProjectPlanBatchRequestBinding>> {
        let rows: Vec<(String, String, String, i64, i64, String, String)> = sqlx::query_as(
            "SELECT b.id,b.project_id,b.authorized_by,b.version,b.graph_version,
                    bt.coordination_snapshot_digest,producer.execution_id
             FROM artifact_execution_producer producer
             JOIN project_plan_batch_execution exec_link
               ON exec_link.execution_id=producer.execution_id
              AND exec_link.task_id=producer.task_id
              AND exec_link.authorization_version=?
             JOIN project_plan_batch b ON b.id=exec_link.batch_id
             JOIN project_plan p ON p.project_id=b.project_id
             JOIN project_plan_batch_task bt
               ON bt.batch_id=b.id AND bt.task_id=producer.task_id
             JOIN task t ON t.id=producer.task_id AND t.project_id=b.project_id
              AND t.version=? AND t.deleted_at IS NULL
             JOIN task_coordination c ON c.task_id=t.id
             JOIN project_plan_batch_coordination_authorization a
               ON a.batch_id=b.id AND a.task_id=t.id
              AND a.authorization_version=c.authorization_version
             JOIN project_member m ON m.project_id=b.project_id
              AND m.user_id=b.authorized_by AND m.role IN ('owner','admin')
             JOIN project owner_project ON owner_project.id=b.project_id
             JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
              AND owner_membership.user_id=owner_project.owner_id AND owner_membership.role='owner'
             WHERE producer.artifact_id=? AND producer.task_id=?
              AND b.state='running' AND b.preparation_complete=1
              AND b.graph_version=p.version AND bt.state='started'
              AND b.authorized_by=? AND c.requested_by=b.authorized_by
              AND c.authorization_version=? AND c.mode='prepare_and_execute'
              AND c.revoked_at IS NULL AND c.team_stale=0
              AND bt.coordination_snapshot_json IS NOT NULL
              AND bt.coordination_snapshot_digest IS NOT NULL
              AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
              AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
              AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
              AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
              AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
              AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
              AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
              AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
              AND a.expected_graph_version=b.graph_version
              AND a.task_title=t.title AND a.task_description IS t.description
              AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
              AND EXISTS(SELECT 1 FROM project_plan_batch_coordination_request scope
                         WHERE scope.batch_id=b.id AND scope.task_id=t.id
                          AND scope.authorization_version=c.authorization_version)
              AND EXISTS(SELECT 1 FROM task_lifecycle l WHERE l.task_id=t.id
                         AND l.state NOT IN ('done','cancelled','merging'))
              AND NOT EXISTS(SELECT 1 FROM task_dispatch d
                             WHERE d.task_id=t.id AND d.state='cancelled')",
        )
        .bind(expected_coordination_version)
        .bind(expected_task_version)
        .bind(source_artifact_id)
        .bind(task_id)
        .bind(user_id)
        .bind(expected_coordination_version)
        .fetch_all(self.db.pool())
        .await?;
        if rows.len() > 1 {
            return Err(db::DbError::ProjectPlanBatchInactive.into());
        }
        Ok(rows.into_iter().next().map(
            |(
                batch_id,
                project_id,
                authorized_by,
                batch_version,
                graph_version,
                snapshot_digest,
                producer_execution_id,
            )| {
                ProjectPlanBatchRequestBinding {
                    project_id,
                    batch_id,
                    authorized_by,
                    expected_batch_version: batch_version,
                    expected_graph_version: graph_version,
                    expected_task_version,
                    expected_coordination_version,
                    coordination_snapshot_digest: snapshot_digest,
                    origin: ProjectPlanBatchRequestOrigin::QualityReplan {
                        source_artifact_id: source_artifact_id.to_owned(),
                        producer_execution_id,
                    },
                }
            },
        ))
    }

    /// Resolves a Human-requested plan revision only from an exact approved
    /// plan Artifact, pending materialization or materialized with its durable
    /// receipt, whose originating request belongs to this running batch.
    pub async fn project_batch_binding_for_approved_plan(
        &self,
        task_id: &str,
        user_id: &str,
        approved_plan_artifact_id: &str,
        expected_task_version: i64,
        expected_coordination_version: i64,
    ) -> Result<Option<ProjectPlanBatchRequestBinding>> {
        let rows: Vec<(String, String, String, i64, i64, String)> = sqlx::query_as(
            "SELECT b.id,b.project_id,b.authorized_by,b.version,b.graph_version,
                    bt.coordination_snapshot_digest
             FROM task_coordination_plan plan
             JOIN project_plan_batch_coordination_request origin
               ON origin.request_id=plan.request_id AND origin.task_id=plan.task_id
             JOIN artifact plan_artifact
               ON plan_artifact.id=plan.artifact_id AND plan_artifact.task_id=plan.task_id
                AND plan_artifact.digest=plan.plan_digest
             JOIN project_plan_batch b ON b.id=origin.batch_id
             JOIN project_plan p ON p.project_id=b.project_id
             JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=plan.task_id
             JOIN task t ON t.id=plan.task_id AND t.project_id=b.project_id
              AND t.version=? AND t.deleted_at IS NULL
             JOIN task_coordination c ON c.task_id=t.id
             JOIN project_plan_batch_coordination_authorization a
               ON a.batch_id=b.id AND a.task_id=t.id
              AND a.authorization_version=c.authorization_version
             JOIN project_member m ON m.project_id=b.project_id
              AND m.user_id=b.authorized_by AND m.role IN ('owner','admin')
             JOIN project owner_project ON owner_project.id=b.project_id
             JOIN project_member owner_membership ON owner_membership.project_id=b.project_id
              AND owner_membership.user_id=owner_project.owner_id AND owner_membership.role='owner'
             WHERE plan.artifact_id=? AND plan.task_id=?
              AND plan.authorization_version=c.authorization_version
              AND ((plan.state='approved' AND NOT EXISTS(
                    SELECT 1 FROM task_coordination_materialization receipt
                    WHERE receipt.plan_artifact_id=plan.artifact_id))
                OR (plan.state='materialized' AND EXISTS(
                    SELECT 1 FROM task_coordination_materialization receipt
                    WHERE receipt.plan_artifact_id=plan.artifact_id
                      AND receipt.task_id=plan.task_id
                      AND receipt.request_id=plan.request_id
                      AND receipt.plan_digest=plan.plan_digest
                      AND receipt.authorization_version=plan.authorization_version)))
              AND b.authorized_by=? AND b.state='running' AND b.preparation_complete=1
              AND b.graph_version=p.version AND bt.state='started'
              AND origin.authorization_version=c.authorization_version
              AND c.requested_by=b.authorized_by AND c.authorization_version=?
              AND c.mode='prepare_and_execute' AND c.revoked_at IS NULL AND c.team_stale=0
              AND bt.coordination_snapshot_json IS NOT NULL
              AND bt.coordination_snapshot_digest IS NOT NULL
              AND c.authorization_version=json_extract(bt.coordination_snapshot_json,'$.authorization_version')
              AND c.team_generation=json_extract(bt.coordination_snapshot_json,'$.team_generation')
              AND c.team_digest=json_extract(bt.coordination_snapshot_json,'$.team_digest')
              AND c.envelope_digest=json_extract(bt.coordination_snapshot_json,'$.envelope_digest')
              AND c.coordinator_task_role_id=json_extract(bt.coordination_snapshot_json,'$.role_id')
              AND c.coordinator_actor_kind=json_extract(bt.coordination_snapshot_json,'$.actor_kind')
              AND c.coordinator_actor_id=json_extract(bt.coordination_snapshot_json,'$.actor_id')
              AND a.project_id=b.project_id AND a.authorized_by=b.authorized_by
              AND a.expected_graph_version=b.graph_version
              AND a.task_title=t.title AND a.task_description IS t.description
              AND a.task_repo_id IS t.repo_id AND a.task_parent_task_id IS t.parent_task_id
              AND EXISTS(SELECT 1 FROM task_lifecycle l WHERE l.task_id=t.id
                         AND l.state NOT IN ('done','cancelled','merging'))
              AND NOT EXISTS(SELECT 1 FROM task_dispatch d
                             WHERE d.task_id=t.id AND d.state='cancelled')",
        )
        .bind(expected_task_version)
        .bind(approved_plan_artifact_id)
        .bind(task_id)
        .bind(user_id)
        .bind(expected_coordination_version)
        .fetch_all(self.db.pool())
        .await?;
        if rows.len() > 1 {
            return Err(db::DbError::ProjectPlanBatchInactive.into());
        }
        Ok(rows.into_iter().next().map(
            |(
                batch_id,
                project_id,
                authorized_by,
                batch_version,
                graph_version,
                snapshot_digest,
            )| {
                ProjectPlanBatchRequestBinding {
                    project_id,
                    batch_id,
                    authorized_by,
                    expected_batch_version: batch_version,
                    expected_graph_version: graph_version,
                    expected_task_version,
                    expected_coordination_version,
                    coordination_snapshot_digest: snapshot_digest,
                    origin: ProjectPlanBatchRequestOrigin::HumanReplan {
                        approved_plan_artifact_id: approved_plan_artifact_id.to_owned(),
                    },
                }
            },
        ))
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
        if coordination.revoked_at.is_some() {
            return Err(coordination_error(
                "coordination.authorization_revoked",
                "Task coordination authorization is revoked",
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
            let (launch_intent_accepted, launch_blockers) = self
                .launch_materialized_work(&task.id, user_id, existing.authorization_version)
                .await?;
            return Ok(MaterializeOutcome {
                plan: self.plan_view(plan).await?,
                request,
                materialization: existing,
                replayed: true,
                launch_intent_accepted,
                launch_blockers,
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
        if coordination.revoked_at.is_some() || coordination.team_stale {
            return Err(coordination_error(
                "coordination.authorization_revoked",
                "Task coordination authorization is revoked or stale",
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
                expected_team_digest: current_team_digest.clone(),
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
        let request = if request.state != TaskCoordinationRequestState::Materialized {
            let now = now_rfc3339();
            let event = self.event(
                "task.coordination_request_materialized",
                "task_coordination_request",
                &request.id,
                "system",
                None,
                &request.task_id,
                json!({
                    "task_id": request.task_id,
                    "request_id": request.id,
                    "plan_artifact_id": artifact_id,
                    "state": TaskCoordinationRequestState::Materialized.to_string(),
                }),
                &now,
            );
            TaskCoordinationRepo::update_task_coordination_request(
                &*self.db,
                db::UpdateTaskCoordinationRequestState {
                    id: request.id.clone(),
                    expected_version: request.version,
                    state: TaskCoordinationRequestState::Materialized,
                    wake_id: None,
                    execution_id: None,
                    plan_artifact_id: Some(artifact_id.to_owned()),
                    failure_code: None,
                    failure_message: None,
                    completed_at: Some(now.clone()),
                    updated_at: now,
                    event: Some(event),
                },
            )
            .await?
        } else {
            request
        };
        let (launch_intent_accepted, launch_blockers) = self
            .launch_materialized_work(&task.id, user_id, materialization.authorization_version)
            .await?;
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
        authorization_version: i64,
    ) -> Result<(bool, Vec<String>)> {
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        let coordination = self.require_coordination(task_id).await?;
        if coordination.authorization_version != authorization_version
            || coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.mode == TaskCoordinationMode::Manual
        {
            return Ok((
                false,
                vec![
                    "coordination.authorization_revoked: Task coordination authorization is revoked or stale"
                        .to_owned(),
                ],
            ));
        }
        let team = self.current_team(task_id).await?;
        if self.team_digest(&team)? != coordination.team_digest {
            return Ok((
                false,
                vec![
                    "coordination.authorization_stale: the Task team changed after the coordination authorization"
                        .to_owned(),
                ],
            ));
        }
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task_id.to_owned()))?;
        let lifecycle_service =
            TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let preparation_blockers = if let Some(service) = self.work_unit_service.as_ref() {
            service
                .prepare_coordination_workspaces(task_id, user_id, authorization_version)
                .await?
        } else {
            Vec::new()
        };
        let preflight = lifecycle_service.launch_preflight(task_id).await?;
        let mut blockers = preflight
            .blockers
            .iter()
            .map(|blocker| format!("{}: {}", blocker.code, blocker.message))
            .collect::<Vec<_>>();
        blockers.extend(preparation_blockers);
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
            || coordination.team_stale
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
                Err(ServiceError::Coordination {
                    code: "coordination.paused",
                    message,
                }) => {
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
Create at most {max_work_units} WorkUnits. Use only TaskRole names and active members listed in the team. A formal review WorkUnit must use a TaskRole with the `formal_review` capability, set review_subject_key to another planned key, and must use a fresh role-isolated native session; the same Agent profile is allowed, but a Human contributor cannot review their own result. The review role must also appear in the envelope's allowed_review_roles by its exact role name. Do not create, rename, or grant TaskRoles, memberships, or capabilities. Do not request external actions or merge. A plan outside the envelope will wait for Human approval.\n\
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
        self.create_quality_request_as(
            task_id,
            CollaborationActorSource::Human(user_id.to_owned()),
            user_id,
            input,
        )
        .await
    }

    /// Reviewer Executions can open the same durable quality request as a
    /// Human. The source Execution must have produced the exact ReviewReport
    /// that justifies the request and must hold the formal_review capability;
    /// the request is attributed to that Actor, not to its owner account.
    pub async fn create_quality_request_as(
        &self,
        task_id: &str,
        source: CollaborationActorSource,
        _authority_user_id: &str,
        input: QualityRequestInput,
    ) -> Result<TaskQualityRequest> {
        if matches!(source, CollaborationActorSource::ProjectCopilot(_)) {
            return Err(coordination_error(
                "coordination.not_authorized",
                "Project Copilot cannot open Task quality requests",
            ));
        }
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
        let (source_kind, source_id, source_digest) = if let Some(artifact_id) =
            input.source_artifact_id.as_deref()
        {
            let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
                .await?
                .filter(|artifact| artifact.task_id == task.id)
                .ok_or_else(|| ServiceError::not_found("artifact", artifact_id.to_owned()))?;
            if artifact.kind != ArtifactKind::ReviewReport {
                return Err(invalid_plan(
                    "a quality request source must be an exact ReviewReport Artifact",
                ));
            }
            match &source {
                CollaborationActorSource::Execution(execution_id) => {
                    let produced_by_source = artifact
                        .execution_producer()
                        .is_some_and(|(producer, _)| producer == execution_id);
                    let execution_is_completed =
                        db::ExecutionRepo::get_by_id(&*self.db, execution_id)
                            .await?
                            .is_some_and(|execution| {
                                execution.task_id == task.id
                                    && execution.status == db::ExecutionStatus::Completed
                            });
                    if !produced_by_source
                        || !execution_is_completed
                        || !crate::role_capabilities::execution_has_capability(
                            &self.db,
                            execution_id,
                            api_types::TaskRoleCapability::FormalReview,
                        )
                        .await?
                    {
                        return Err(coordination_error(
                            "coordination.not_authorized",
                            "a reviewer quality request requires its own exact completed ReviewReport and formal_review authority",
                        ));
                    }
                }
                CollaborationActorSource::Human(_) => {}
                CollaborationActorSource::ProjectCopilot(_) => {
                    return Err(coordination_error(
                        "coordination.not_authorized",
                        "Project Copilot cannot open Task quality requests",
                    ));
                }
            }
            let digest = artifact.digest.clone().unwrap_or_else(|| {
                sha256_hex(artifact.content.as_deref().unwrap_or_default().as_bytes())
            });
            ("review_report".to_owned(), artifact.id, digest)
        } else {
            if matches!(source, CollaborationActorSource::Execution(_)) {
                return Err(invalid_plan(
                    "a reviewer quality request requires its exact ReviewReport source",
                ));
            }
            (
                "human".to_owned(),
                actor.id().to_owned(),
                sha256_hex(input.summary.as_bytes()),
            )
        };
        let (route_id, route_kind, route_ref, source_task_role_id) =
            if source_kind == "review_report" {
                let review_line_id: Option<String> = sqlx::query_scalar(
                    "SELECT MIN(line_id) FROM work_unit_review_line_revision
                     WHERE review_report_artifact_id = ?",
                )
                .bind(&source_id)
                .fetch_one(self.db.pool())
                .await?;
                let source_task_role_id: Option<String> = sqlx::query_scalar(
                    "SELECT authority.task_role_id
                     FROM artifact_execution_producer producer
                     JOIN execution_task_role_authority authority
                       ON authority.execution_id = producer.execution_id
                     WHERE producer.artifact_id = ?",
                )
                .bind(&source_id)
                .fetch_optional(self.db.pool())
                .await?;
                if let Some(line_id) = review_line_id {
                    (
                        format!("review_line:{line_id}"),
                        "review_line".to_owned(),
                        line_id,
                        source_task_role_id,
                    )
                } else {
                    (
                        format!(
                            "source:{}:{}:{}",
                            hex::encode_upper(source_kind.as_bytes()),
                            hex::encode_upper(source_id.as_bytes()),
                            source_digest
                        ),
                        "source".to_owned(),
                        source_id.clone(),
                        source_task_role_id,
                    )
                }
            } else {
                (
                    format!(
                        "source:{}:{}:{}",
                        hex::encode_upper(source_kind.as_bytes()),
                        hex::encode_upper(source_id.as_bytes()),
                        source_digest
                    ),
                    "source".to_owned(),
                    source_id.clone(),
                    None,
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
        let request_id = new_uuid_v4();
        let now = now_rfc3339();
        let event = self.event(
            "task.quality_requested",
            "task_quality_request",
            &request_id,
            &actor.kind().to_string(),
            Some(actor.id()),
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
        let state = if input.category == TaskQualityCategory::HumanDecision {
            TaskQualityState::RequiresHuman
        } else {
            TaskQualityState::Requested
        };
        let (initial_requires_human_reason_code, initial_requires_human_reason) =
            if input.category == TaskQualityCategory::HumanDecision {
                (
                    Some("coordination.requires_human".to_owned()),
                    Some("the quality request explicitly requires a Human decision".to_owned()),
                )
            } else {
                (None, None)
            };
        let write = TaskCoordinationRepo::create_task_quality_request(
            &*self.db,
            db::CreateTaskQualityRequestInput {
                id: request_id.clone(),
                task_id: task.id.clone(),
                source_kind,
                source_id,
                category: input.category,
                target_kind: input.target_kind,
                target_task_role_id: input.target_task_role_id.clone(),
                target_actor: input.target_actor.clone(),
                summary: input.summary.clone(),
                findings_json,
                resolution_condition: input.resolution_condition.clone(),
                state,
                handoff_id: None,
                work_unit_id: None,
                source_digest,
                route_id,
                route_kind,
                route_ref,
                source_task_role_id,
                initial_requires_human_reason_code,
                initial_requires_human_reason,
                request_digest,
                idempotency_key: input.idempotency_key.clone(),
                created_by: actor.clone(),
                created_at: now.clone(),
                event,
            },
        )
        .await?;
        if write.replayed {
            return Ok(write.request);
        }
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        if write.request.state == TaskQualityState::RequiresHuman {
            return Ok(write.request);
        }
        // The handoff is an informational notification, not the effect
        // authority. Creating it with a deterministic ID after the request
        // commits makes retries and concurrent consumers converge on one
        // notification without consuming another budget round.
        let target = match (input.target_actor.clone(), target_role.as_ref()) {
            (Some(actor), _) => db::CollaborationTarget::Actor(actor),
            (None, Some(role)) => db::CollaborationTarget::Role(role.id.clone()),
            _ => db::CollaborationTarget::Task,
        };
        let handoff = self
            .collaboration
            .create_handoff_with_id(
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
                        TaskQualityCategory::HumanDecision => db::HandoffIntent::DecisionRequest,
                    },
                    parent_execution_id: None,
                    expected_policy_ref: Some("forge.task-quality-request".to_owned()),
                    artifact_ids: input.source_artifact_id.iter().cloned().collect(),
                },
                deterministic_quality_handoff_id(&write.request.id)?,
            )
            .await?;
        self.attach_quality_handoff(&task.id, &write.request.id, &handoff.id)
            .await
    }

    async fn attach_quality_handoff(
        &self,
        task_id: &str,
        request_id: &str,
        handoff_id: &str,
    ) -> Result<db::TaskQualityRequest> {
        // A durable consumer may advance this request after creation commits
        // and before the informational Handoff is attached. Retry against the
        // current row and preserve any state the consumer has already reached.
        for _ in 0..8 {
            let request = TaskCoordinationRepo::get_task_quality_request(&*self.db, request_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("quality request", request_id))?;
            if request.task_id != task_id {
                return Err(ServiceError::not_found("quality request", request_id));
            }
            if let Some(existing_handoff_id) = request.handoff_id.as_deref() {
                if existing_handoff_id == handoff_id {
                    return Ok(request);
                }
                return Err(coordination_error(
                    "coordination.handoff_conflict",
                    "the quality request already links a different Handoff",
                ));
            }

            let state = if request.state == TaskQualityState::Requested {
                TaskQualityState::Notified
            } else {
                request.state
            };
            let state_label = state.to_string();
            let now = now_rfc3339();
            let update = TaskCoordinationRepo::update_task_quality_request_state(
                &*self.db,
                db::UpdateTaskQualityRequestState {
                    id: request.id.clone(),
                    expected_version: request.version,
                    state,
                    handoff_id: Some(Some(handoff_id.to_owned())),
                    work_unit_id: None,
                    coordination_request_id: None,
                    resolution_kind: None,
                    resolution_ref: None,
                    resolution_digest: None,
                    requires_human_reason_code: None,
                    requires_human_reason: None,
                    updated_at: now.clone(),
                    event: Some(self.event(
                        "task.quality_request_notified",
                        "task_quality_request",
                        request_id,
                        "system",
                        None,
                        task_id,
                        json!({
                            "task_id": task_id,
                            "quality_request_id": request_id,
                            "handoff_id": handoff_id,
                            "state": state_label,
                        }),
                        &now,
                    )),
                },
            )
            .await;
            match update {
                Ok(updated) => return Ok(updated),
                Err(db::DbError::VersionConflict) => continue,
                Err(error) => return Err(error.into()),
            }
        }

        let request = TaskCoordinationRepo::get_task_quality_request(&*self.db, request_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("quality request", request_id))?;
        if request.handoff_id.as_deref() == Some(handoff_id) {
            return Ok(request);
        }
        Err(ServiceError::Db(db::DbError::VersionConflict))
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
            db::UpdateTaskQualityRequestState {
                id: request.id,
                expected_version,
                state,
                handoff_id: None,
                work_unit_id: None,
                coordination_request_id: None,
                resolution_kind: None,
                resolution_ref: None,
                resolution_digest: None,
                requires_human_reason_code: None,
                requires_human_reason: None,
                updated_at: now,
                event: Some(event),
            },
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
            && input.kind != TaskInterventionKind::Instruction
        {
            return Err(coordination_error(
                "coordination.not_authorized",
                "Project Copilot may only record an instruction; decisions, pause, cancel, revocation, and replanning remain Human actions",
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
            return self
                .resume_recorded_intervention(&task, &source, user_id, &write.intervention, &input)
                .await;
        }
        if let Some(event) = write.event.as_ref() {
            self.domain_events.publish_committed(event);
        }
        match self
            .apply_intervention(&task, &source, user_id, &write.intervention, &input, false)
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

    /// Replay of a durable intervention resumes its pending effect instead of
    /// returning a `recorded` row with no effect. Every effect below is
    /// idempotent by an intervention-derived identity or receipt, so a crash
    /// before or after the effect converges to one applied intervention.
    async fn resume_recorded_intervention(
        &self,
        task: &Task,
        source: &CollaborationActorSource,
        user_id: &str,
        intervention: &TaskIntervention,
        input: &InterventionInput,
    ) -> Result<TaskIntervention> {
        if matches!(
            intervention.state,
            TaskInterventionState::Applied
                | TaskInterventionState::Rejected
                | TaskInterventionState::RequiresHuman
        ) {
            return Ok(intervention.clone());
        }
        match self
            .apply_intervention(task, source, user_id, intervention, input, true)
            .await
        {
            Ok(()) => {
                self.set_intervention_state(
                    intervention,
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
                    intervention,
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
        resuming: bool,
    ) -> Result<()> {
        if !resuming {
            if let Some(expected_task_version) = input.expected_task_version {
                if task.version != expected_task_version {
                    return Err(db::DbError::TaskVersionConflict {
                        expected: expected_task_version,
                        actual: task.version,
                    }
                    .into());
                }
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
                    .create_message_with_id(
                        source.clone(),
                        CreateMessageInput {
                            task_id: task.id.clone(),
                            target: db::CollaborationTarget::Task,
                            work_unit_id: None,
                            body: instruction.to_owned(),
                            artifact_ids: Vec::new(),
                        },
                        deterministic_intervention_message_id(&intervention.id)?,
                    )
                    .await?;
                Ok(())
            }
            TaskInterventionKind::Pause
            | TaskInterventionKind::Resume
            | TaskInterventionKind::Cancel => {
                let expected_lifecycle_version = if resuming {
                    TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?
                        .version
                } else {
                    input.expected_lifecycle_version.ok_or_else(|| {
                        invalid_plan("intervention requires expected_lifecycle_version")
                    })?
                };
                let task_version = if resuming {
                    TaskRepo::get_by_id(&*self.db, &task.id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?
                        .version
                } else {
                    task.version
                };
                let lifecycle =
                    TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
                let requested_by = ApiActorRef::Human(user_id.to_owned());
                if resuming {
                    let dispatch_key = format!("intervention:{}", intervention.id);
                    if TaskLifecycleRepo::get_task_dispatch_receipt(
                        &*self.db,
                        &task.id,
                        &dispatch_key,
                    )
                    .await?
                    .is_some()
                    {
                        return Ok(());
                    }
                }
                match input.kind {
                    TaskInterventionKind::Pause => {
                        lifecycle
                            .pause_task(
                                &task.id,
                                task_version,
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
                                task_version,
                                expected_lifecycle_version,
                                &requested_by,
                                &format!("intervention:{}", intervention.id),
                            )
                            .await?;
                    }
                    TaskInterventionKind::Cancel => {
                        let lifecycle_key = format!("intervention-cancel:{}", intervention.id);
                        let cancel_transition_exists = resuming
                            && TaskLifecycleRepo::has_task_lifecycle_transition(
                                &*self.db,
                                &task.id,
                                &lifecycle_key,
                            )
                            .await?;
                        if !cancel_transition_exists {
                            lifecycle
                                .transition_at_lifecycle_version(
                                    crate::task_lifecycle::TransitionLifecycleInput {
                                        task_id: task.id.clone(),
                                        expected_task_version: task_version,
                                        to_state: TaskLifecycleState::Cancelled,
                                        cause: crate::task_lifecycle::LifecycleCause::Actor(
                                            api_types::Actor::User {
                                                user_id: Some(user_id.to_owned()),
                                                source: api_types::UserActionSource::Api,
                                            },
                                        ),
                                        reason_kind: Some("intervention".to_owned()),
                                        reason_ref: Some(intervention.id.clone()),
                                        idempotency_key: lifecycle_key,
                                    },
                                    expected_lifecycle_version,
                                )
                                .await?;
                        }
                        lifecycle
                            .record_cancelled_dispatch_after_cancel(
                                &task.id,
                                task_version,
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
                let batch_binding =
                    if let Some(approved_plan_artifact_id) = input.plan_artifact_id.as_deref() {
                        self.project_batch_binding_for_approved_plan(
                            &task.id,
                            user_id,
                            approved_plan_artifact_id,
                            brief.expected_task_version,
                            brief.expected_coordination_version,
                        )
                        .await?
                    } else {
                        None
                    };
                if resuming {
                    if let Some(existing) =
                        TaskCoordinationRepo::get_task_coordination_request_by_key(
                            &*self.db,
                            &task.id,
                            &brief.idempotency_key,
                        )
                        .await?
                    {
                        let batch_bound =
                            TaskCoordinationRepo::is_project_plan_batch_coordination_request(
                                &*self.db,
                                &existing.id,
                            )
                            .await?;
                        if !batch_bound && batch_binding.is_none() {
                            return Ok(());
                        }
                    }
                }
                if let Some(binding) = batch_binding {
                    self.request_project_batch_turn(&task.id, user_id, brief, binding)
                        .await?;
                } else {
                    self.request_turn(&task.id, user_id, brief).await?;
                }
                Ok(())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
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
                "the plan must contain between 1 and 64 WorkUnits",
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

    #[allow(clippy::too_many_arguments)]
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
                if unit.requires_integration {
                    return Err(invalid_plan(
                        "a formal review WorkUnit cannot require repository integration",
                    ));
                }
                if !keys.contains(review_key) || review_key == unit.key {
                    return Err(invalid_plan(
                        "a review subject must name a different WorkUnit in this plan",
                    ));
                }
                validate_review_role(role, envelope)?;
                let reviewed = plan
                    .work_units
                    .iter()
                    .find(|candidate| candidate.key == review_key)
                    .ok_or_else(|| invalid_plan("review subject key is missing"))?;
                if unit.assigned_actor.is_some()
                    && unit.assigned_actor == reviewed.assigned_actor
                    && matches!(unit.assigned_actor.as_ref(), Some(ApiActorRef::Human(_)))
                    && envelope.require_independent_review
                {
                    return Err(invalid_plan(
                        "a Human contributor cannot review their own result",
                    ));
                }
                if reviewed.review_subject_key.is_some() {
                    return Err(invalid_plan("review WorkUnits cannot review each other"));
                }
            } else if role
                .capabilities
                .contains(&TaskRoleCapability::FormalReview)
            {
                return Err(invalid_plan(
                    "a formal_review WorkUnit requires an explicit review_subject_key",
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
        validate_dependency_graph(plan)?;
        for unit in &plan.work_units {
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
                            .any(|role| role == &unit.role)
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

    #[allow(clippy::too_many_arguments)]
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

/// A formal review is authorized by the TaskRole capability and its exact
/// role name in the envelope, never by the literal `reviewer`.
fn validate_review_role(role: &TeamRoleView, envelope: &CoordinationEnvelope) -> Result<()> {
    if !role
        .capabilities
        .contains(&TaskRoleCapability::FormalReview)
    {
        return Err(invalid_plan(
            "the review TaskRole lacks formal_review capability",
        ));
    }
    if !envelope
        .allowed_review_roles
        .iter()
        .any(|allowed| allowed == &role.role)
    {
        return Err(unsupported_item(
            "the envelope does not allow the review TaskRole",
        ));
    }
    Ok(())
}

/// Kahn's algorithm distinguishes a real back edge from a second path through
/// an already-settled shared ancestor. A single per-root `seen` set rejects a
/// valid diamond as a cycle.
fn validate_dependency_graph(plan: &WorkUnitPlan) -> Result<()> {
    let mut indegree = BTreeMap::new();
    let mut dependents: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for unit in &plan.work_units {
        indegree.insert(unit.key.as_str(), unit.depends_on.len());
        for dependency in &unit.depends_on {
            dependents
                .entry(dependency.as_str())
                .or_default()
                .push(unit.key.as_str());
        }
    }
    let mut ready = indegree
        .iter()
        .filter_map(|(key, degree)| (*degree == 0).then_some(*key))
        .collect::<Vec<_>>();
    let mut ordered = 0;
    while let Some(key) = ready.pop() {
        ordered += 1;
        if let Some(next) = dependents.get(key) {
            for dependent in next {
                let Some(degree) = indegree.get_mut(dependent) else {
                    return Err(invalid_plan(
                        "a WorkUnit dependency is not part of this plan",
                    ));
                };
                *degree = degree
                    .checked_sub(1)
                    .ok_or_else(|| invalid_plan("the plan dependency graph is inconsistent"))?;
                if *degree == 0 {
                    ready.push(dependent);
                }
            }
        }
    }
    if ordered != plan.work_units.len() {
        return Err(invalid_plan("the plan dependency graph contains a cycle"));
    }
    Ok(())
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

fn deterministic_intervention_message_id(intervention_id: &str) -> Result<String> {
    let digest = Sha256::digest(
        [
            b"forge.task-coordination.intervention-message.v1".as_slice(),
            intervention_id.as_bytes(),
        ]
        .concat(),
    );
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(uuid::Uuid::from_bytes(bytes).to_string())
}

fn deterministic_quality_handoff_id(request_id: &str) -> Result<String> {
    let digest = Sha256::digest(
        [
            b"forge.task-coordination.quality-handoff.v1".as_slice(),
            request_id.as_bytes(),
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
    fn historical_envelopes_keep_direct_routing_and_new_defaults_mediate_findings() {
        let mut historical = serde_json::to_value(CoordinationEnvelope::default()).unwrap();
        historical
            .as_object_mut()
            .unwrap()
            .remove("orchestrator_reviews_findings");
        let parsed: CoordinationEnvelope = serde_json::from_value(historical).unwrap();
        assert!(!parsed.orchestrator_reviews_findings);
        assert!(CoordinationEnvelope::default().orchestrator_reviews_findings);
    }

    #[test]
    fn default_envelope_is_structurally_safe() {
        let envelope = CoordinationEnvelope::default();
        envelope.validate().expect("default envelope");
        assert_eq!(envelope.max_work_units, 16);
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

    fn plan_with(dependencies: &[(&str, &[&str])]) -> WorkUnitPlan {
        WorkUnitPlan {
            schema_version: 1,
            request_id: "request-1".to_owned(),
            task_id: "task-1".to_owned(),
            task_version: 1,
            authorization_version: 1,
            team_digest: "team".to_owned(),
            envelope_digest: "envelope".to_owned(),
            objective_digest: "objective".to_owned(),
            work_units: dependencies
                .iter()
                .map(|(key, depends_on)| PlanWorkUnit {
                    key: (*key).to_owned(),
                    title: (*key).to_owned(),
                    scope: (*key).to_owned(),
                    role: "implementer".to_owned(),
                    assigned_actor: None,
                    parent_key: None,
                    depends_on: depends_on.iter().map(|key| (*key).to_owned()).collect(),
                    requires_integration: false,
                    execution_instructions: None,
                    review_subject_key: None,
                    acceptance: Vec::new(),
                    artifact_inputs: Vec::new(),
                })
                .collect(),
            delivery: None,
            review: None,
            limits: None,
        }
    }

    #[test]
    fn dependency_graph_accepts_a_diamond() {
        let plan = plan_with(&[("a", &[]), ("b", &["a"]), ("c", &["a"]), ("d", &["b", "c"])]);
        validate_dependency_graph(&plan).expect("a diamond is acyclic");
    }

    #[test]
    fn dependency_graph_accepts_redundant_transitive_edges() {
        let plan = plan_with(&[("a", &[]), ("b", &["a"]), ("c", &["a", "b"])]);
        validate_dependency_graph(&plan).expect("redundant ancestor edge is acyclic");
    }

    #[test]
    fn dependency_graph_rejects_a_real_cycle() {
        let plan = plan_with(&[("a", &["b"]), ("b", &["a"])]);
        let error = validate_dependency_graph(&plan).expect_err("cycle is rejected");
        assert!(error.to_string().contains("cycle"));
    }

    fn review_role(name: &str, formal_review: bool) -> TeamRoleView {
        TeamRoleView {
            id: format!("role-{name}"),
            role: name.to_owned(),
            display_label: None,
            coordination_mode: Some(db::CoordinationMode::Independent),
            capabilities: if formal_review {
                vec![TaskRoleCapability::FormalReview]
            } else {
                Vec::new()
            },
            configuration: TaskRoleConfiguration::default(),
            version: 1,
            members: Vec::new(),
        }
    }

    #[test]
    fn formal_review_uses_capability_and_exact_envelope_name() {
        let custom = review_role("revisor", true);
        let mut envelope = CoordinationEnvelope::default();
        assert!(validate_review_role(&custom, &envelope).is_err());
        envelope.allowed_review_roles = vec!["revisor".to_owned()];
        validate_review_role(&custom, &envelope).expect("custom review role is allowed");
        let ordinary = review_role("implementer", false);
        assert!(validate_review_role(&ordinary, &envelope).is_err());
        assert!(validate_review_role(&review_role("reviewer", true), &envelope).is_err());
    }

    #[test]
    fn intervention_messages_and_quality_handoffs_are_stable() {
        assert_eq!(
            deterministic_intervention_message_id("intervention-1").expect("id"),
            deterministic_intervention_message_id("intervention-1").expect("id")
        );
        assert_ne!(
            deterministic_intervention_message_id("intervention-1").expect("id"),
            deterministic_intervention_message_id("intervention-2").expect("id")
        );
        assert_eq!(
            deterministic_quality_handoff_id("request-1").expect("id"),
            deterministic_quality_handoff_id("request-1").expect("id")
        );
    }
}
