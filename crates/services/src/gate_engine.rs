use std::{collections::BTreeSet, sync::Arc};

use db::{
    new_uuid_v4, now_rfc3339, ActorKind, ActorRef, ArtifactKind, CollaborationRepo,
    CreateDomainEvent, CreateGate, CreateGatePolicyRevision, DecisionOutcome, DomainEvent,
    ExecutionPurpose, ExecutionRepo, ExecutionStatus, Gate, GateEvaluation, GateEvaluationInput,
    GateEvaluationOutcome, GateEvaluationWrite, GatePolicyRevision, GateRepo, GateScopeKind,
    ProposalStatus, RoleMembershipRepo, RoleMembershipStatus, SqliteDb,
    TaskIntegrationOperationKind, TaskIntegrationOperationRepo, TaskLifecycleRepo,
    TaskLifecycleState, TaskLifecycleTransitionFact, TaskRepo, TaskRoleRepo, ValidationRunRepo,
    ValidationRunStatus, WorkUnitRepo, WorkUnitStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    task_lifecycle::{LifecycleCause, TaskLifecycleService, TransitionLifecycleInput},
    Result, ServiceError,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatePolicyDocument {
    pub schema_version: u32,
    #[serde(default)]
    pub scope_requirement: Option<GateScopeRequirement>,
    #[serde(default)]
    pub review: Option<ReviewSetPolicy>,
    #[serde(default)]
    pub validations: Vec<ValidationRequirement>,
    #[serde(default)]
    pub decisions: Vec<DecisionRequirement>,
    #[serde(default)]
    pub work_units: Vec<WorkUnitRequirement>,
}

/// Operation-scoped Gates pin one exact merge operation or immutable
/// lifecycle transition. Task and WorkUnit scopes use their typed input
/// requirements instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum GateScopeRequirement {
    MergeOperation {
        operation_id: String,
        version: i64,
        expected_status: String,
        gate_evaluation_id: String,
    },
    LifecycleOperation {
        transition_id: String,
        from_state: String,
        to_state: String,
        from_version: i64,
        to_version: i64,
        cause_kind: String,
        cause_ref: Option<String>,
        gate_evaluation_id: Option<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSelectionMode {
    OneAcceptable,
    AllRequired,
    AtLeast,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSetPolicy {
    pub mode: ReviewSelectionMode,
    pub required_count: u32,
    pub human_required: bool,
    #[serde(default = "default_true")]
    pub allow_humans: bool,
    #[serde(default = "default_true")]
    pub allow_agents: bool,
    #[serde(default)]
    pub allowed_actor_refs: Vec<ActorRef>,
    #[serde(default)]
    pub task_role_snapshot: Option<TaskRoleReviewerSnapshot>,
    pub candidates: Vec<ReviewReportRequirement>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TaskRoleReviewerSnapshot {
    pub task_role_id: String,
    pub role: String,
    pub version: i64,
    pub membership_digest: String,
    pub actor_refs: Vec<ActorRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewReportRequirement {
    pub artifact_id: String,
    pub digest: String,
    pub expected_actor: Option<ActorRef>,
    pub required: bool,
    pub subject: ReviewSubjectExpectation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSubjectExpectation {
    pub workspace_id: Option<String>,
    pub base_commit_sha: Option<String>,
    pub head_commit_sha: Option<String>,
    pub workspace_snapshot_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValidationRequirement {
    pub validation_run_id: String,
    pub evidence_id: String,
    pub evidence_digest: String,
    pub check_identity: String,
    pub config_digest: String,
    pub workspace_id: String,
    pub commit_sha: String,
    pub workspace_snapshot_digest: String,
    pub required_outcome: ValidationRunStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionRequirement {
    pub proposal_id: String,
    pub proposal_version: i64,
    pub decision_id: String,
    pub outcome: DecisionOutcome,
    pub policy_ref: Option<String>,
    pub policy_version: Option<i64>,
    pub policy_digest: Option<String>,
    pub permitted_deciders: Vec<ActorRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkUnitRequirement {
    pub work_unit_id: String,
    pub version: i64,
    pub dependency_digest: String,
    pub execution_id: String,
    pub execution_result_sha: String,
    pub require_integration: bool,
    pub integration_id: Option<String>,
    pub integration_version: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateEvaluationResult {
    pub schema_version: u32,
    pub outcome: GateEvaluationOutcome,
    pub issues: Vec<String>,
    pub missing_inputs: Vec<String>,
    pub checked_inputs: usize,
}

#[derive(Clone)]
pub struct GateEngine {
    db: Arc<SqliteDb>,
    event_bus: Arc<events::EventBus>,
}

#[derive(Debug, Clone, Serialize)]
struct InputDraft {
    input_kind: String,
    input_id: String,
    input_version: i64,
    input_digest: String,
    producer_ref: Option<String>,
    subject_json: String,
    status: String,
}

impl GateEngine {
    pub fn new(db: Arc<SqliteDb>, event_bus: Arc<events::EventBus>) -> Self {
        Self { db, event_bus }
    }

    pub async fn create_gate(
        &self,
        task_id: &str,
        gate_kind: &str,
        scope_kind: GateScopeKind,
        scope_id: &str,
    ) -> Result<Gate> {
        validate_text("task_id", task_id, 128)?;
        validate_text("gate_kind", gate_kind, 64)?;
        validate_text("scope_id", scope_id, 128)?;
        let id = new_uuid_v4();
        let now = now_rfc3339();
        let event = gate_event(
            "gate.created",
            "gate",
            &id,
            task_id,
            None,
            json!({"gate_id": id, "gate_kind": gate_kind, "scope_kind": scope_kind, "scope_id": scope_id}),
            &now,
        );
        let write = GateRepo::create_gate(
            &*self.db,
            CreateGate {
                id,
                task_id: task_id.to_owned(),
                gate_kind: gate_kind.to_owned(),
                scope_kind,
                scope_id: scope_id.to_owned(),
                created_at: now,
                event,
            },
        )
        .await?;
        crate::DomainEventService::publish_committed_hint(&self.event_bus, &write.event);
        Ok(write.record)
    }

    pub async fn create_gate_with_initial_policy(
        &self,
        task_id: &str,
        gate_kind: &str,
        scope_kind: GateScopeKind,
        scope_id: &str,
        mut policy: GatePolicyDocument,
    ) -> Result<(Gate, GatePolicyRevision)> {
        validate_text("task_id", task_id, 128)?;
        validate_text("gate_kind", gate_kind, 64)?;
        validate_text("scope_id", scope_id, 128)?;
        normalize_policy(&mut policy)?;
        let gate_id = new_uuid_v4();
        let now = now_rfc3339();
        let gate = Gate {
            id: gate_id.clone(),
            task_id: task_id.to_owned(),
            gate_kind: gate_kind.to_owned(),
            scope_kind,
            scope_id: scope_id.to_owned(),
            active_policy_revision: Some(1),
            created_at: now.clone(),
        };
        self.validate_policy_scope(&gate, &policy).await?;
        self.validate_role_snapshots(&gate, &policy).await?;
        let policy_json = serde_json::to_string(&policy)
            .map_err(|error| invalid(format!("Gate policy cannot be serialized: {error}")))?;
        let policy_digest = sha256(policy_json.as_bytes());
        let policy_revision_id = format!("{gate_id}:policy:1");
        let gate_created_event = gate_event(
            "gate.created",
            "gate",
            &gate_id,
            task_id,
            None,
            json!({"gate_id": gate_id, "gate_kind": gate_kind, "scope_kind": scope_kind, "scope_id": scope_id}),
            &now,
        );
        let policy_event = gate_event(
            "gate.policy_revised",
            "gate",
            &policy_revision_id,
            task_id,
            None,
            json!({"gate_id": gate_id, "revision": 1, "policy_digest": policy_digest}),
            &now,
        );
        let (gate_write, policy_write) = GateRepo::create_gate_with_initial_policy(
            &*self.db,
            CreateGate {
                id: gate_id.clone(),
                task_id: task_id.to_owned(),
                gate_kind: gate_kind.to_owned(),
                scope_kind,
                scope_id: scope_id.to_owned(),
                created_at: now.clone(),
                event: gate_created_event,
            },
            CreateGatePolicyRevision {
                gate_id,
                expected_active_revision: None,
                revision: 1,
                schema_version: i64::from(policy.schema_version),
                policy_json,
                policy_digest,
                created_at: now,
                event: policy_event,
            },
        )
        .await?;
        crate::DomainEventService::publish_committed_hint(&self.event_bus, &gate_write.event);
        crate::DomainEventService::publish_committed_hint(&self.event_bus, &policy_write.event);
        Ok((gate_write.record, policy_write.record))
    }

    pub async fn revise_policy(
        &self,
        gate_id: &str,
        expected_active_revision: Option<i64>,
        mut policy: GatePolicyDocument,
    ) -> Result<GatePolicyRevision> {
        let gate = GateRepo::get_gate(&*self.db, gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("gate", gate_id.to_owned()))?;
        normalize_policy(&mut policy)?;
        self.validate_policy_scope(&gate, &policy).await?;
        self.validate_role_snapshots(&gate, &policy).await?;
        let policy_json = serde_json::to_string(&policy)
            .map_err(|error| invalid(format!("Gate policy cannot be serialized: {error}")))?;
        let policy_digest = sha256(policy_json.as_bytes());
        let revision = expected_active_revision.unwrap_or(0) + 1;
        let now = now_rfc3339();
        let policy_revision_id = format!("{gate_id}:policy:{revision}");
        let event = gate_event(
            "gate.policy_revised",
            "gate",
            &policy_revision_id,
            &gate.task_id,
            None,
            json!({"gate_id": gate_id, "revision": revision, "policy_digest": policy_digest}),
            &now,
        );
        let write = GateRepo::create_gate_policy_revision(
            &*self.db,
            CreateGatePolicyRevision {
                gate_id: gate_id.to_owned(),
                expected_active_revision,
                revision,
                schema_version: i64::from(policy.schema_version),
                policy_json,
                policy_digest,
                created_at: now,
                event,
            },
        )
        .await?;
        crate::DomainEventService::publish_committed_hint(&self.event_bus, &write.event);
        Ok(write.record)
    }

    pub async fn evaluate_active(&self, gate_id: &str) -> Result<GateEvaluationWrite> {
        let gate = GateRepo::get_gate(&*self.db, gate_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("gate", gate_id.to_owned()))?;
        let revision = gate
            .active_policy_revision
            .ok_or_else(|| ServiceError::invalid_operation("Gate has no active policy revision"))?;
        self.evaluate_revision(gate, revision).await
    }

    pub async fn evaluate_revision(
        &self,
        gate: Gate,
        revision: i64,
    ) -> Result<GateEvaluationWrite> {
        self.evaluate_revision_with_cause(gate, revision, None)
            .await
    }

    async fn evaluate_revision_with_cause(
        &self,
        gate: Gate,
        revision: i64,
        causation_id: Option<String>,
    ) -> Result<GateEvaluationWrite> {
        let policy_revision = GateRepo::get_gate_policy_revision(&*self.db, &gate.id, revision)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("gate policy revision", format!("{}:{revision}", gate.id))
            })?;
        let mut issues = Vec::new();
        let mut missing = Vec::new();
        let mut inputs = Vec::new();
        let (outcome, policy) = if policy_revision.schema_version != 1 {
            issues.push("unknown_policy_schema".to_owned());
            (GateEvaluationOutcome::Indeterminate, None)
        } else if sha256(policy_revision.policy_json.as_bytes()) != policy_revision.policy_digest {
            issues.push("policy_digest_mismatch".to_owned());
            (GateEvaluationOutcome::Indeterminate, None)
        } else {
            match serde_json::from_str::<GatePolicyDocument>(&policy_revision.policy_json) {
                Ok(policy) if policy.schema_version == 1 => {
                    (GateEvaluationOutcome::Unsatisfied, Some(policy))
                }
                Ok(_) => {
                    issues.push("unknown_policy_schema".to_owned());
                    (GateEvaluationOutcome::Indeterminate, None)
                }
                Err(_) => {
                    issues.push("invalid_policy_shape".to_owned());
                    (GateEvaluationOutcome::Indeterminate, None)
                }
            }
        };

        if let Some(policy) = policy.as_ref() {
            if !policy_has_requirements(policy) {
                issues.push("empty_policy".to_owned());
            } else {
                self.evaluate_scope_requirement(
                    &gate,
                    policy,
                    &mut inputs,
                    &mut missing,
                    &mut issues,
                )
                .await?;
                self.evaluate_review(&gate, policy, &mut inputs, &mut missing, &mut issues)
                    .await?;
                self.evaluate_validations(&gate, policy, &mut inputs, &mut missing, &mut issues)
                    .await?;
                self.evaluate_decisions(&gate, policy, &mut inputs, &mut missing, &mut issues)
                    .await?;
                self.evaluate_work_units(&gate, policy, &mut inputs, &mut missing, &mut issues)
                    .await?;
            }
        }

        inputs.sort_by(|left, right| {
            (&left.input_kind, &left.input_id).cmp(&(&right.input_kind, &right.input_id))
        });
        issues.sort();
        issues.dedup();
        missing.sort();
        missing.dedup();
        let final_outcome = if policy.is_none() {
            outcome
        } else if issues.is_empty() && missing.is_empty() {
            GateEvaluationOutcome::Satisfied
        } else {
            GateEvaluationOutcome::Unsatisfied
        };
        let input_material = json!({
            "policy_digest": policy_revision.policy_digest,
            "inputs": inputs,
            "issues": issues,
            "missing": missing,
        });
        let input_digest = sha256(input_material.to_string().as_bytes());
        let result = GateEvaluationResult {
            schema_version: 1,
            outcome: final_outcome,
            issues,
            missing_inputs: missing,
            checked_inputs: inputs.len(),
        };
        let result_json = serde_json::to_string(&result).map_err(|error| {
            invalid(format!(
                "Gate evaluation result cannot be serialized: {error}"
            ))
        })?;
        let evaluation_id = new_uuid_v4();
        let evaluated_at = now_rfc3339();
        let event = gate_event(
            "gate.evaluated",
            "gate_evaluation",
            &evaluation_id,
            &gate.task_id,
            causation_id,
            json!({
                "evaluation_id": evaluation_id,
                "gate_id": gate.id,
                "task_id": gate.task_id,
                "policy_revision": revision,
                "outcome": final_outcome,
                "input_digest": input_digest,
            }),
            &evaluated_at,
        );
        let inputs = inputs
            .into_iter()
            .enumerate()
            .map(|(ordinal, input)| GateEvaluationInput {
                evaluation_id: evaluation_id.clone(),
                ordinal: ordinal as i64,
                input_kind: input.input_kind,
                input_id: input.input_id,
                input_version: input.input_version,
                input_digest: input.input_digest,
                producer_ref: input.producer_ref,
                subject_json: input.subject_json,
                status: input.status,
            })
            .collect::<Vec<_>>();
        let write = GateRepo::create_gate_evaluation(
            &*self.db,
            db::StoreGateEvaluation {
                evaluation: GateEvaluation {
                    id: evaluation_id,
                    gate_id: gate.id,
                    task_id: gate.task_id,
                    policy_revision: revision,
                    outcome: final_outcome,
                    input_digest,
                    result_json,
                    evaluated_at,
                },
                inputs,
                event,
            },
        )
        .await?;
        if let Some(event) = write.event.as_ref() {
            crate::DomainEventService::publish_committed_hint(&self.event_bus, event);
        }
        Ok(write)
    }

    /// Durable domain events trigger exact-policy reevaluation. EventBus hints
    /// only wake the existing consumer; this method itself reads the durable
    /// event and never selects a latest review, validation, approval or result.
    pub async fn process_domain_event(&self, event: &DomainEvent) -> Result<usize> {
        if event.event_type == "gate.evaluated" {
            self.apply_evaluation_event(event).await?;
            return Ok(0);
        }
        if event.scope_type != "task" || !gate_fact_event(event) {
            return Ok(0);
        }
        let policies = GateRepo::list_active_gate_policies(&*self.db, &event.scope_id).await?;
        let mut evaluated = 0;
        for (gate, policy) in policies {
            // If this source event already produced a durable GateEvaluation,
            // its own `gate.evaluated` event owns the lifecycle effect. Do not
            // rebuild an evaluation from newer facts while replaying the
            // original source event after a crash.
            if GateRepo::get_gate_evaluation_for_cause(&*self.db, &gate.id, &event.id)
                .await?
                .is_some()
            {
                continue;
            }
            let write = self
                .evaluate_revision_with_cause(gate.clone(), policy.revision, Some(event.id.clone()))
                .await?;
            if write.event.is_some() {
                evaluated += 1;
            }
        }
        Ok(evaluated)
    }

    async fn apply_evaluation_event(&self, event: &DomainEvent) -> Result<()> {
        if event.entity_type != "gate_evaluation" || event.scope_type != "task" {
            return Ok(());
        }
        let Some(evaluation) = GateRepo::get_gate_evaluation(&*self.db, &event.entity_id).await?
        else {
            return Ok(());
        };
        let payload: Value = serde_json::from_str(&event.payload_json)
            .map_err(|error| invalid(format!("invalid GateEvaluation event payload: {error}")))?;
        if evaluation.id != event.entity_id
            || evaluation.task_id != event.scope_id
            || payload.get("evaluation_id").and_then(Value::as_str) != Some(evaluation.id.as_str())
            || payload.get("gate_id").and_then(Value::as_str) != Some(evaluation.gate_id.as_str())
            || payload.get("policy_revision").and_then(Value::as_i64)
                != Some(evaluation.policy_revision)
            || payload.get("input_digest").and_then(Value::as_str)
                != Some(evaluation.input_digest.as_str())
        {
            return Ok(());
        }
        let Some(gate) = GateRepo::get_gate(&*self.db, &evaluation.gate_id).await? else {
            return Ok(());
        };
        if gate.task_id != evaluation.task_id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
        {
            return Ok(());
        }
        self.apply_merge_readiness_evaluation(&gate, &evaluation)
            .await
    }

    async fn apply_merge_readiness_evaluation(
        &self,
        gate: &Gate,
        evaluation: &GateEvaluation,
    ) -> Result<()> {
        if evaluation.gate_id != gate.id
            || evaluation.task_id != gate.task_id
            || gate.gate_kind != "merge_readiness"
            || gate.scope_kind != GateScopeKind::Task
            || gate.scope_id != gate.task_id
            || gate.active_policy_revision != Some(evaluation.policy_revision)
        {
            return Ok(());
        }
        if !GateRepo::is_latest_gate_evaluation(
            &*self.db,
            &gate.id,
            evaluation.policy_revision,
            &evaluation.id,
        )
        .await?
        {
            return Ok(());
        }
        if !GateRepo::gate_evaluation_inputs_are_current(&*self.db, &evaluation.id).await? {
            // Facts that can change after evaluation are fenced again before
            // lifecycle movement. The event is safe to consume; the durable
            // event for the changed fact will produce a new exact evaluation.
            return Ok(());
        }
        let Some(mut task) = TaskRepo::get_by_id(&*self.db, &gate.task_id, false).await? else {
            return Ok(());
        };
        let mut lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("task lifecycle", task.id.clone()))?;
        // A new exact input set or policy revision invalidates the previous
        // readiness receipt. Re-enter Active first; if the new frozen
        // evaluation is satisfied, bind the next transition to that receipt.
        if lifecycle.state == TaskLifecycleState::ReadyToMerge
            && lifecycle.reason_ref.as_deref() != Some(evaluation.id.as_str())
        {
            let demoted =
                TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                    .transition(TransitionLifecycleInput {
                        task_id: task.id.clone(),
                        expected_task_version: task.version,
                        to_state: TaskLifecycleState::Active,
                        cause: LifecycleCause::GateEvaluation(evaluation.id.clone()),
                        reason_kind: Some("merge_readiness_rechecked".to_owned()),
                        reason_ref: Some(evaluation.id.clone()),
                        idempotency_key: format!(
                            "gate-evaluation:{}:invalidate-ready-to-merge",
                            evaluation.id
                        ),
                    })
                    .await?;
            task = demoted.task;
            lifecycle = demoted.lifecycle;
        }
        if evaluation.outcome == GateEvaluationOutcome::Satisfied
            && lifecycle.state == TaskLifecycleState::Active
        {
            let idempotency_key = format!("gate-evaluation:{}:ready-to-merge", evaluation.id);
            if !TaskLifecycleRepo::has_task_lifecycle_transition(
                &*self.db,
                &task.id,
                &idempotency_key,
            )
            .await?
            {
                TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                    .transition(TransitionLifecycleInput {
                        task_id: task.id.clone(),
                        expected_task_version: task.version,
                        to_state: TaskLifecycleState::ReadyToMerge,
                        cause: LifecycleCause::GateEvaluation(evaluation.id.clone()),
                        reason_kind: Some("merge_readiness_satisfied".to_owned()),
                        reason_ref: Some(evaluation.id.clone()),
                        idempotency_key,
                    })
                    .await?;
            }
        }
        Ok(())
    }

    async fn validate_policy_scope(&self, gate: &Gate, policy: &GatePolicyDocument) -> Result<()> {
        match (&gate.scope_kind, &policy.scope_requirement) {
            (GateScopeKind::Task | GateScopeKind::WorkUnit, None) => {}
            (
                GateScopeKind::MergeOperation,
                Some(GateScopeRequirement::MergeOperation { operation_id, .. }),
            ) if operation_id == &gate.scope_id => {
                let operation = TaskIntegrationOperationRepo::get_by_id(&*self.db, operation_id)
                    .await?
                    .ok_or_else(|| invalid("Gate merge-operation scope is missing"))?;
                if operation.task_id != gate.task_id
                    || operation.kind != TaskIntegrationOperationKind::TaskMerge
                {
                    return Err(invalid(
                        "Gate merge-operation scope is cross-Task or not a TaskMerge",
                    ));
                }
            }
            (
                GateScopeKind::LifecycleOperation,
                Some(GateScopeRequirement::LifecycleOperation { transition_id, .. }),
            ) if transition_id == &gate.scope_id => {
                let transition =
                    TaskLifecycleRepo::get_task_lifecycle_transition_fact(&*self.db, transition_id)
                        .await?
                        .ok_or_else(|| invalid("Gate lifecycle-operation scope is missing"))?;
                if transition.task_id != gate.task_id {
                    return Err(invalid(
                        "Gate lifecycle-operation scope belongs to another Task",
                    ));
                }
            }
            _ => {
                return Err(invalid(
                    "Gate policy scope requirement does not match its exact Gate scope",
                ));
            }
        }
        if gate.gate_kind == "merge_readiness"
            && (gate.scope_kind != GateScopeKind::Task || gate.scope_id != gate.task_id)
        {
            return Err(invalid(
                "merge-readiness Gate must be scoped to its exact Task",
            ));
        }
        if gate.gate_kind == "merge_readiness"
            && policy
                .decisions
                .iter()
                .any(|decision| decision.outcome != DecisionOutcome::Approve)
        {
            return Err(invalid(
                "merge-readiness Decision requirements must require approval",
            ));
        }
        if gate.gate_kind == "merge_readiness"
            && policy
                .validations
                .iter()
                .any(|validation| validation.required_outcome != ValidationRunStatus::Passed)
        {
            return Err(invalid(
                "merge-readiness ValidationRun requirements must require PASS",
            ));
        }
        if gate.scope_kind == GateScopeKind::WorkUnit
            && !policy
                .work_units
                .iter()
                .any(|unit| unit.work_unit_id == gate.scope_id)
        {
            return Err(invalid(
                "WorkUnit-scoped Gate policy must pin that exact WorkUnit",
            ));
        }
        if policy.review.is_none()
            && policy.validations.is_empty()
            && policy.decisions.is_empty()
            && policy.work_units.is_empty()
            && policy.scope_requirement.is_none()
        {
            return Err(invalid(
                "Gate policy must contain at least one deterministic requirement",
            ));
        }
        Ok(())
    }

    async fn validate_role_snapshots(
        &self,
        gate: &Gate,
        policy: &GatePolicyDocument,
    ) -> Result<()> {
        let Some(snapshot) = policy
            .review
            .as_ref()
            .and_then(|review| review.task_role_snapshot.as_ref())
        else {
            return Ok(());
        };
        let role = TaskRoleRepo::get_by_id(&*self.db, &snapshot.task_role_id)
            .await?
            .ok_or_else(|| invalid("Gate TaskRole reviewer snapshot is missing"))?;
        if role.task_id != gate.task_id
            || role.role != snapshot.role
            || role.version != snapshot.version
        {
            return Err(invalid(
                "Gate TaskRole reviewer snapshot is stale or cross-Task",
            ));
        }
        let members = RoleMembershipRepo::list_by_role(&*self.db, &role.id, false)
            .await?
            .into_iter()
            .filter(|member| member.status == RoleMembershipStatus::Active)
            .map(|member| member.actor_ref())
            .collect::<Vec<_>>();
        if normalized_actor_refs(members) != normalized_actor_refs(snapshot.actor_refs.clone())
            || actor_set_digest(&snapshot.actor_refs) != snapshot.membership_digest
        {
            return Err(invalid(
                "Gate TaskRole membership snapshot does not match current membership",
            ));
        }
        Ok(())
    }

    async fn evaluate_scope_requirement(
        &self,
        gate: &Gate,
        policy: &GatePolicyDocument,
        inputs: &mut Vec<InputDraft>,
        missing: &mut Vec<String>,
        issues: &mut Vec<String>,
    ) -> Result<()> {
        match (&gate.scope_kind, &policy.scope_requirement) {
            (GateScopeKind::Task | GateScopeKind::WorkUnit, None) => {}
            (
                GateScopeKind::MergeOperation,
                Some(GateScopeRequirement::MergeOperation {
                    operation_id,
                    version,
                    expected_status,
                    gate_evaluation_id,
                }),
            ) => {
                let Some(operation) =
                    TaskIntegrationOperationRepo::get_by_id(&*self.db, operation_id).await?
                else {
                    missing.push(format!("merge_operation:{operation_id}"));
                    return Ok(());
                };
                let subject = json!({
                "operation_id": operation.id,
                "task_id": operation.task_id,
                "kind": operation.kind.to_string(),
                    "owner_id": operation.owner_id,
                    "version": operation.version,
                    "status": operation.status.to_string(),
                    "gate_evaluation_id": operation.gate_evaluation_id,
                    "created_at": operation.created_at,
                    "updated_at": operation.updated_at,
                    "finished_at": operation.finished_at,
                });
                let subject_json = subject.to_string();
                let exact = operation.id == gate.scope_id
                    && operation.task_id == gate.task_id
                    && operation.kind == TaskIntegrationOperationKind::TaskMerge
                    && operation.version == *version
                    && operation.status.to_string() == *expected_status
                    && operation.gate_evaluation_id.as_deref() == Some(gate_evaluation_id.as_str());
                if !exact {
                    issues.push(format!("merge_operation_scope_mismatch:{}", operation.id));
                }
                inputs.push(InputDraft {
                    input_kind: "merge_operation".to_owned(),
                    input_id: operation.id.clone(),
                    input_version: operation.version,
                    input_digest: sha256(subject_json.as_bytes()),
                    producer_ref: Some(gate_evaluation_id.clone()),
                    subject_json,
                    status: operation.status.to_string(),
                });
            }
            (
                GateScopeKind::LifecycleOperation,
                Some(GateScopeRequirement::LifecycleOperation {
                    transition_id,
                    from_state,
                    to_state,
                    from_version,
                    to_version,
                    cause_kind,
                    cause_ref,
                    gate_evaluation_id,
                }),
            ) => {
                let Some(transition) =
                    TaskLifecycleRepo::get_task_lifecycle_transition_fact(&*self.db, transition_id)
                        .await?
                else {
                    missing.push(format!("lifecycle_operation:{transition_id}"));
                    return Ok(());
                };
                let subject = lifecycle_transition_subject(&transition);
                let subject_json = subject.to_string();
                let exact = transition.id == gate.scope_id
                    && transition.task_id == gate.task_id
                    && transition.from_state.to_string() == *from_state
                    && transition.to_state.to_string() == *to_state
                    && transition.from_version == *from_version
                    && transition.to_version == *to_version
                    && transition.cause_kind == *cause_kind
                    && transition.cause_ref == *cause_ref
                    && transition.gate_evaluation_id == *gate_evaluation_id;
                if !exact {
                    issues.push(format!(
                        "lifecycle_operation_scope_mismatch:{}",
                        transition.id
                    ));
                }
                inputs.push(InputDraft {
                    input_kind: "lifecycle_operation".to_owned(),
                    input_id: transition.id.clone(),
                    input_version: transition.to_version,
                    input_digest: sha256(subject_json.as_bytes()),
                    producer_ref: transition.cause_ref.clone(),
                    subject_json,
                    status: transition.to_state.to_string(),
                });
            }
            _ => issues.push("gate_scope_requirement_mismatch".to_owned()),
        }
        Ok(())
    }

    async fn evaluate_review(
        &self,
        gate: &Gate,
        policy: &GatePolicyDocument,
        inputs: &mut Vec<InputDraft>,
        missing: &mut Vec<String>,
        issues: &mut Vec<String>,
    ) -> Result<()> {
        let Some(requirement) = &policy.review else {
            return Ok(());
        };
        let task_role_current = if let Some(snapshot) = &requirement.task_role_snapshot {
            match TaskRoleRepo::get_by_id(&*self.db, &snapshot.task_role_id).await? {
                None => {
                    missing.push(format!("task_role:{}", snapshot.task_role_id));
                    Some(false)
                }
                Some(role) if role.task_id != gate.task_id => {
                    issues.push(format!(
                        "task_role_snapshot_wrong_task:{}",
                        snapshot.task_role_id
                    ));
                    Some(false)
                }
                Some(role) => {
                    let memberships =
                        RoleMembershipRepo::list_by_role(&*self.db, &role.id, true).await?;
                    let mut membership_rows = memberships
                        .iter()
                        .map(|membership| {
                            json!({
                                "id": membership.id,
                                "version": membership.version,
                                "actor": membership.actor_ref(),
                                "status": membership.status.to_string(),
                                "ended_at": membership.ended_at,
                            })
                        })
                        .collect::<Vec<_>>();
                    membership_rows.sort_by_key(|row| {
                        row.get("id")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_owned()
                    });
                    let active_members = normalized_actor_refs(
                        memberships
                            .iter()
                            .filter(|membership| membership.status == RoleMembershipStatus::Active)
                            .map(|membership| membership.actor_ref())
                            .collect(),
                    );
                    let current = task_role_snapshot_matches(
                        snapshot,
                        gate.task_id.as_str(),
                        &role,
                        &active_members,
                    );
                    let subject = json!({
                        "task_role_id": role.id,
                        "role": role.role,
                        "role_version": role.version,
                        "membership_rows": membership_rows,
                        "active_members": active_members,
                        "policy_membership_digest": snapshot.membership_digest,
                    });
                    let subject_json = subject.to_string();
                    inputs.push(InputDraft {
                        input_kind: "task_role_snapshot".to_owned(),
                        input_id: role.id.clone(),
                        input_version: role.version,
                        input_digest: sha256(subject_json.as_bytes()),
                        producer_ref: None,
                        subject_json,
                        status: if current { "current" } else { "stale" }.to_owned(),
                    });
                    if !current {
                        issues.push(format!(
                            "task_role_snapshot_stale:{}",
                            snapshot.task_role_id
                        ));
                    }
                    Some(current)
                }
            }
        } else {
            None
        };
        // A quorum is a set of reviewers, not a count of report rows. One
        // Actor can produce multiple exact reports, but those reports must
        // never satisfy a multi-reviewer policy by themselves.
        let mut passing_reviewers = PassingReviewers::default();
        let mut required_failed = false;
        for candidate in &requirement.candidates {
            let Some(artifact) =
                CollaborationRepo::get_artifact(&*self.db, &candidate.artifact_id).await?
            else {
                record_review_candidate_failure(
                    candidate,
                    &mut required_failed,
                    missing,
                    issues,
                    None,
                );
                continue;
            };
            let Some((execution_id, producer_actor)) = artifact.execution_producer() else {
                record_review_candidate_failure(
                    candidate,
                    &mut required_failed,
                    missing,
                    issues,
                    Some(format!("invalid_review_producer:{}", candidate.artifact_id)),
                );
                continue;
            };
            if artifact.task_id != gate.task_id
                || artifact.kind != ArtifactKind::ReviewReport
                || artifact.digest.as_deref() != Some(candidate.digest.as_str())
                || artifact.content_ref.is_some()
            {
                record_review_candidate_failure(
                    candidate,
                    &mut required_failed,
                    missing,
                    issues,
                    Some(format!(
                        "review_report_identity_mismatch:{}",
                        candidate.artifact_id
                    )),
                );
                continue;
            }
            let Some(execution) = ExecutionRepo::get_by_id(&*self.db, execution_id).await? else {
                record_review_candidate_failure(
                    candidate,
                    &mut required_failed,
                    missing,
                    issues,
                    Some(format!(
                        "review_execution_missing:{}",
                        candidate.artifact_id
                    )),
                );
                continue;
            };
            let content: Value = artifact
                .content
                .as_deref()
                .and_then(|raw| serde_json::from_str(raw).ok())
                .unwrap_or(Value::Null);
            let verdict = content.get("verdict").and_then(Value::as_str).unwrap_or("");
            let subject = content.get("subject").cloned().unwrap_or(Value::Null);
            let subject_matches = subject.get("task_id").and_then(Value::as_str)
                == Some(gate.task_id.as_str())
                && subject.get("review_execution_id").and_then(Value::as_str)
                    == Some(execution.id.as_str())
                && subject.get("workspace_id").and_then(Value::as_str)
                    == candidate.subject.workspace_id.as_deref()
                && subject.get("base_commit_sha").and_then(Value::as_str)
                    == candidate.subject.base_commit_sha.as_deref()
                && subject.get("head_commit_sha").and_then(Value::as_str)
                    == candidate.subject.head_commit_sha.as_deref()
                && subject
                    .get("workspace_snapshot_digest")
                    .and_then(Value::as_str)
                    == candidate.subject.workspace_snapshot_digest.as_deref();
            let actor_ok = execution.actor_ref().as_ref() == Some(producer_actor)
                && candidate
                    .expected_actor
                    .as_ref()
                    .is_none_or(|expected| expected == producer_actor)
                && (requirement.allowed_actor_refs.is_empty()
                    || requirement
                        .allowed_actor_refs
                        .iter()
                        .any(|actor| actor == producer_actor));
            let actor_kind_ok = match producer_actor.kind() {
                ActorKind::Human => requirement.allow_humans,
                ActorKind::Agent => requirement.allow_agents,
            };
            let scope_matches = match gate.scope_kind {
                GateScopeKind::Task => true,
                GateScopeKind::WorkUnit => policy
                    .work_units
                    .iter()
                    .find(|unit| unit.work_unit_id == gate.scope_id)
                    .is_some_and(|unit| {
                        // The immutable Execution is admitted against the open
                        // WorkUnit version. Completing that WorkUnit advances
                        // its aggregate version once, so the exact producer
                        // version is the preceding revision.
                        unit.version
                            .checked_sub(1)
                            .is_some_and(|execution_version| {
                                execution.work_unit_id.as_deref() == Some(gate.scope_id.as_str())
                                    && execution.work_unit_version == Some(execution_version)
                            })
                    }),
                GateScopeKind::MergeOperation | GateScopeKind::LifecycleOperation => true,
            };
            let task_role_ok = requirement
                .task_role_snapshot
                .as_ref()
                .is_none_or(|snapshot| {
                    task_role_current == Some(true) && snapshot.actor_refs.contains(producer_actor)
                });
            let producer_ok = execution.task_id == gate.task_id
                && execution.role == "reviewer"
                && execution.purpose == Some(ExecutionPurpose::Review)
                && execution.status == ExecutionStatus::Completed
                && actor_kind_ok
                && actor_ok
                && task_role_ok
                && scope_matches
                && subject_matches
                && matches!(verdict, "pass" | "request_changes" | "questions");
            if !producer_ok {
                record_review_candidate_failure(
                    candidate,
                    &mut required_failed,
                    missing,
                    issues,
                    Some(format!(
                        "review_report_not_acceptable:{}",
                        candidate.artifact_id
                    )),
                );
            } else if verdict == "pass" {
                passing_reviewers.insert(producer_actor);
            } else if candidate.required {
                required_failed = true;
                issues.push(format!(
                    "required_review_not_passed:{}",
                    candidate.artifact_id
                ));
            }
            if producer_ok {
                inputs.push(InputDraft {
                    input_kind: "review_report".to_owned(),
                    input_id: artifact.id.clone(),
                    input_version: 1,
                    input_digest: candidate.digest.clone(),
                    producer_ref: Some(execution.id.clone()),
                    subject_json: subject.to_string(),
                    status: verdict.to_owned(),
                });
            }
        }
        let threshold = match requirement.mode {
            ReviewSelectionMode::OneAcceptable => 1,
            ReviewSelectionMode::AllRequired => requirement.candidates.len() as u32,
            ReviewSelectionMode::AtLeast => requirement.required_count,
        };
        if passing_reviewers.count() < threshold as usize {
            issues.push("review_quorum_not_satisfied".to_owned());
        }
        if requirement.human_required && !passing_reviewers.has_human() {
            issues.push("human_review_required".to_owned());
        }
        if required_failed {
            issues.push("required_review_failed".to_owned());
        }
        Ok(())
    }

    async fn evaluate_validations(
        &self,
        gate: &Gate,
        policy: &GatePolicyDocument,
        inputs: &mut Vec<InputDraft>,
        missing: &mut Vec<String>,
        issues: &mut Vec<String>,
    ) -> Result<()> {
        for requirement in &policy.validations {
            if gate.gate_kind == "merge_readiness"
                && requirement.required_outcome != ValidationRunStatus::Passed
            {
                issues.push(format!(
                    "merge_readiness_requires_validation_pass:{}",
                    requirement.validation_run_id
                ));
            }
            let Some(run) =
                ValidationRunRepo::get_validation_run(&*self.db, &requirement.validation_run_id)
                    .await?
            else {
                missing.push(format!("validation_run:{}", requirement.validation_run_id));
                continue;
            };
            let Some(evidence) =
                ValidationRunRepo::get_evidence(&*self.db, &requirement.evidence_id).await?
            else {
                missing.push(format!("evidence:{}", requirement.evidence_id));
                continue;
            };
            if run.task_id != gate.task_id || evidence.task_id != gate.task_id {
                issues.push(format!("validation_input_wrong_task:{}", run.id));
                continue;
            }
            if gate.scope_kind == GateScopeKind::WorkUnit
                && run.work_unit_id.as_deref() != Some(gate.scope_id.as_str())
            {
                issues.push(format!("validation_input_wrong_scope:{}", run.id));
                continue;
            }
            let evidence_is_bound = evidence.producer_validation_run_id == run.id
                && evidence.digest == requirement.evidence_digest
                && evidence.kind == "deterministic_check_output";
            let identity_matches = run.check_identity == requirement.check_identity
                && run.config_digest == requirement.config_digest
                && run.workspace_id == requirement.workspace_id
                && run.commit_sha == requirement.commit_sha
                && run.workspace_snapshot_digest == requirement.workspace_snapshot_digest
                && run.status == requirement.required_outcome;
            if !evidence_is_bound {
                issues.push(format!("validation_evidence_mismatch:{}", evidence.id));
            }
            if !identity_matches {
                issues.push(format!("validation_subject_or_outcome_mismatch:{}", run.id));
            }
            let subject = json!({
                "check_identity": run.check_identity,
                "config_digest": run.config_digest,
                "workspace_id": run.workspace_id,
                "commit_sha": run.commit_sha,
                "workspace_snapshot_digest": run.workspace_snapshot_digest,
            });
            let run_digest = sha256(
                json!({
                    "id": run.id,
                    "task_id": run.task_id,
                    "check_identity": run.check_identity,
                    "config_digest": run.config_digest,
                    "workspace_id": run.workspace_id,
                    "commit_sha": run.commit_sha,
                    "workspace_snapshot_digest": run.workspace_snapshot_digest,
                    "status": run.status,
                    "exit_code": run.exit_code,
                    "finished_at": run.finished_at,
                })
                .to_string()
                .as_bytes(),
            );
            inputs.push(InputDraft {
                input_kind: "validation_run".to_owned(),
                input_id: run.id.clone(),
                input_version: 1,
                input_digest: run_digest,
                producer_ref: None,
                subject_json: json!({
                    "check_identity": run.check_identity,
                    "config_digest": run.config_digest,
                    "workspace_id": run.workspace_id,
                    "commit_sha": run.commit_sha,
                    "workspace_snapshot_digest": run.workspace_snapshot_digest,
                })
                .to_string(),
                status: run.status.to_string(),
            });
            if evidence_is_bound {
                inputs.push(InputDraft {
                    input_kind: "evidence".to_owned(),
                    input_id: evidence.id,
                    input_version: 1,
                    input_digest: evidence.digest,
                    producer_ref: Some(run.id),
                    subject_json: subject.to_string(),
                    status: run.status.to_string(),
                });
            }
        }
        Ok(())
    }

    async fn evaluate_decisions(
        &self,
        gate: &Gate,
        policy: &GatePolicyDocument,
        inputs: &mut Vec<InputDraft>,
        missing: &mut Vec<String>,
        issues: &mut Vec<String>,
    ) -> Result<()> {
        for requirement in &policy.decisions {
            if gate.gate_kind == "merge_readiness"
                && requirement.outcome != DecisionOutcome::Approve
            {
                issues.push(format!(
                    "merge_readiness_requires_approval_decision:{}",
                    requirement.decision_id
                ));
            }
            let Some(decision) =
                CollaborationRepo::get_decision(&*self.db, &requirement.decision_id).await?
            else {
                missing.push(format!("decision:{}", requirement.decision_id));
                continue;
            };
            let Some(proposal) =
                CollaborationRepo::get_proposal(&*self.db, &requirement.proposal_id).await?
            else {
                missing.push(format!("proposal:{}", requirement.proposal_id));
                continue;
            };
            let policy_matches = decision.policy_ref == requirement.policy_ref
                && decision.policy_version == requirement.policy_version
                && decision.policy_digest == requirement.policy_digest;
            let deciders_match = !decision.actors.is_empty()
                && (requirement.permitted_deciders.is_empty()
                    || decision
                        .actors
                        .iter()
                        .all(|actor| requirement.permitted_deciders.contains(actor)));
            let exact = decision.task_id == gate.task_id
                && proposal.task_id == gate.task_id
                && decision.proposal_id == proposal.id
                && proposal.content_version == requirement.proposal_version
                && decision.proposal_version == requirement.proposal_version
                && decision.outcome == requirement.outcome
                && (gate.gate_kind != "merge_readiness"
                    || requirement.outcome == DecisionOutcome::Approve)
                && proposal.status == ProposalStatus::Resolved
                && policy_matches
                && deciders_match;
            if !exact {
                issues.push(format!("decision_authority_mismatch:{}", decision.id));
            }
            let subject = json!({
                "proposal_id": decision.proposal_id,
                "policy_ref": decision.policy_ref,
                "policy_version": decision.policy_version,
                "policy_digest": decision.policy_digest,
                "actors": decision.actors,
            });
            inputs.push(InputDraft {
                input_kind: "decision".to_owned(),
                input_id: decision.id.clone(),
                input_version: decision.proposal_version,
                input_digest: sha256(
                    json!({
                        "id": decision.id,
                        "task_id": decision.task_id,
                        "proposal_id": decision.proposal_id,
                        "proposal_version": decision.proposal_version,
                        "outcome": decision.outcome,
                        "policy_ref": decision.policy_ref,
                        "policy_version": decision.policy_version,
                        "policy_digest": decision.policy_digest,
                        "actors": decision.actors,
                    })
                    .to_string()
                    .as_bytes(),
                ),
                producer_ref: Some(proposal.id),
                subject_json: subject.to_string(),
                status: decision.outcome.to_string(),
            });
        }
        Ok(())
    }

    async fn evaluate_work_units(
        &self,
        gate: &Gate,
        policy: &GatePolicyDocument,
        inputs: &mut Vec<InputDraft>,
        missing: &mut Vec<String>,
        issues: &mut Vec<String>,
    ) -> Result<()> {
        for requirement in &policy.work_units {
            let Some(unit) = WorkUnitRepo::get_by_id(&*self.db, &requirement.work_unit_id).await?
            else {
                missing.push(format!("work_unit:{}", requirement.work_unit_id));
                continue;
            };
            let dependencies = WorkUnitRepo::list_dependencies(&*self.db, &unit.id).await?;
            let dependency_snapshot = dependencies
                .iter()
                .map(|dependency| {
                    json!({
                        "work_unit_id": dependency.work_unit_id,
                        "depends_on_work_unit_id": dependency.depends_on_work_unit_id,
                        "satisfied": dependency.satisfied,
                    })
                })
                .collect::<Vec<_>>();
            let dependency_digest = sha256(json!(dependency_snapshot).to_string().as_bytes());
            if unit.task_id != gate.task_id
                || unit.version != requirement.version
                || unit.status != WorkUnitStatus::Completed
            {
                issues.push(format!("work_unit_not_exactly_completed:{}", unit.id));
                continue;
            }
            if dependencies.iter().any(|dependency| !dependency.satisfied)
                || dependency_digest != requirement.dependency_digest
            {
                issues.push(format!("work_unit_dependency_state_mismatch:{}", unit.id));
            }
            let Some(execution) =
                ExecutionRepo::get_by_id(&*self.db, &requirement.execution_id).await?
            else {
                missing.push(format!("execution:{}", requirement.execution_id));
                continue;
            };
            let execution_version = requirement.version.checked_sub(1);
            let execution_exact = execution.task_id == gate.task_id
                && execution.work_unit_id.as_deref() == Some(unit.id.as_str())
                && execution.work_unit_version == execution_version
                && execution.status == ExecutionStatus::Completed
                && execution.after_sha.as_deref()
                    == Some(requirement.execution_result_sha.as_str());
            if !execution_exact {
                issues.push(format!("work_unit_execution_mismatch:{}", execution.id));
                continue;
            }
            inputs.push(InputDraft {
                input_kind: "work_unit".to_owned(),
                input_id: unit.id.clone(),
                input_version: unit.version,
                input_digest: sha256(
                    json!({
                        "id": unit.id,
                        "task_id": unit.task_id,
                        "status": unit.status,
                        "version": unit.version,
                        "dependency_snapshot": dependency_snapshot,
                    })
                    .to_string()
                    .as_bytes(),
                ),
                producer_ref: None,
                subject_json: json!({
                    "execution_id": execution.id,
                    "execution_result_sha": execution.after_sha,
                    "dependencies": dependency_snapshot,
                })
                .to_string(),
                status: unit.status.to_string(),
            });
            inputs.push(InputDraft {
                input_kind: "execution".to_owned(),
                input_id: execution.id.clone(),
                input_version: 1,
                input_digest: sha256(
                    json!({
                        "id": execution.id,
                        "task_id": execution.task_id,
                        "work_unit_id": execution.work_unit_id,
                        "work_unit_version": execution.work_unit_version,
                        "status": execution.status.to_string(),
                        "after_sha": execution.after_sha,
                    })
                    .to_string()
                    .as_bytes(),
                ),
                producer_ref: Some(unit.id.clone()),
                subject_json: json!({
                    "work_unit_id": execution.work_unit_id,
                    "work_unit_version": execution.work_unit_version,
                    "result_sha": execution.after_sha,
                })
                .to_string(),
                status: execution.status.to_string(),
            });
            if requirement.require_integration {
                let Some(integration_id) = requirement.integration_id.as_deref() else {
                    issues.push(format!("work_unit_integration_required:{}", unit.id));
                    continue;
                };
                let Some(integration) =
                    WorkUnitRepo::get_integration_by_id(&*self.db, integration_id).await?
                else {
                    missing.push(format!("work_unit_integration:{integration_id}"));
                    continue;
                };
                if integration.task_id != gate.task_id
                    || integration.work_unit_id != unit.id
                    || integration.execution_id != execution.id
                    || integration.source_sha != requirement.execution_result_sha
                    || integration.version != requirement.integration_version.unwrap_or(-1)
                    || integration.outcome != db::WorkUnitIntegrationOutcome::Success
                    || integration.target_after_sha.as_deref().is_none()
                {
                    issues.push(format!("work_unit_integration_mismatch:{integration_id}"));
                    continue;
                }
                inputs.push(InputDraft {
                    input_kind: "work_unit_integration".to_owned(),
                    input_id: integration.id.clone(),
                    input_version: integration.version,
                    input_digest: sha256(
                        json!({
                            "id": integration.id,
                            "task_id": integration.task_id,
                            "work_unit_id": integration.work_unit_id,
                            "execution_id": integration.execution_id,
                            "source_sha": integration.source_sha,
                            "target_before_sha": integration.target_before_sha,
                            "target_after_sha": integration.target_after_sha,
                            "outcome": integration.outcome,
                        })
                        .to_string()
                        .as_bytes(),
                    ),
                    producer_ref: Some(execution.id),
                    subject_json: json!({
                        "work_unit_id": unit.id,
                        "source_sha": integration.source_sha,
                        "workspace_id": integration.target_workspace_id,
                        "target_before_sha": integration.target_before_sha,
                        "commit_sha": integration.target_after_sha,
                    })
                    .to_string(),
                    status: integration.outcome.to_string(),
                });
            } else if requirement.integration_id.is_some()
                || requirement.integration_version.is_some()
            {
                issues.push(format!("unexpected_work_unit_integration:{}", unit.id));
            }
        }
        Ok(())
    }
}

fn record_review_candidate_failure(
    candidate: &ReviewReportRequirement,
    required_failed: &mut bool,
    missing: &mut Vec<String>,
    issues: &mut Vec<String>,
    issue: Option<String>,
) {
    if !candidate.required {
        return;
    }
    *required_failed = true;
    if let Some(issue) = issue {
        issues.push(issue);
    } else {
        missing.push(format!("review_report:{}", candidate.artifact_id));
    }
}

fn normalize_policy(policy: &mut GatePolicyDocument) -> Result<()> {
    if policy.schema_version != 1 {
        return Err(invalid(
            "new Gate policies must use supported schema version 1",
        ));
    }
    if let Some(review) = &mut policy.review {
        if review.candidates.is_empty() || (!review.allow_humans && !review.allow_agents) {
            return Err(invalid(
                "review Gate needs candidates and an acceptable Actor kind",
            ));
        }
        match review.mode {
            ReviewSelectionMode::OneAcceptable if review.required_count != 1 => {
                return Err(invalid(
                    "one_acceptable reviewer policy requires required_count=1",
                ));
            }
            ReviewSelectionMode::AllRequired
                if review.required_count as usize != review.candidates.len() =>
            {
                return Err(invalid(
                    "all_required reviewer policy must require every candidate",
                ));
            }
            ReviewSelectionMode::AllRequired
                if review
                    .candidates
                    .iter()
                    .any(|candidate| !candidate.required) =>
            {
                return Err(invalid(
                    "all_required reviewer policy cannot contain optional candidates",
                ));
            }
            ReviewSelectionMode::AtLeast
                if review.required_count == 0
                    || review.required_count as usize > review.candidates.len() =>
            {
                return Err(invalid(
                    "N-of-M reviewer count is outside the candidate set",
                ));
            }
            _ => {}
        }
        review
            .candidates
            .sort_by(|a, b| a.artifact_id.cmp(&b.artifact_id));
        if duplicate_by(
            review
                .candidates
                .iter()
                .map(|item| item.artifact_id.as_str()),
        ) {
            return Err(invalid(
                "review Gate policy contains duplicate ReviewReport IDs",
            ));
        }
        review.allowed_actor_refs =
            normalized_actor_refs(std::mem::take(&mut review.allowed_actor_refs));
        if let Some(snapshot) = &mut review.task_role_snapshot {
            snapshot.actor_refs = normalized_actor_refs(std::mem::take(&mut snapshot.actor_refs));
        }
    }
    policy
        .validations
        .sort_by(|a, b| a.validation_run_id.cmp(&b.validation_run_id));
    if duplicate_by(
        policy
            .validations
            .iter()
            .map(|item| item.validation_run_id.as_str()),
    ) {
        return Err(invalid("Gate policy contains duplicate ValidationRun IDs"));
    }
    policy
        .decisions
        .sort_by(|a, b| a.decision_id.cmp(&b.decision_id));
    if duplicate_by(
        policy
            .decisions
            .iter()
            .map(|item| item.decision_id.as_str()),
    ) {
        return Err(invalid("Gate policy contains duplicate Decision IDs"));
    }
    for decision in &mut policy.decisions {
        decision.permitted_deciders =
            normalized_actor_refs(std::mem::take(&mut decision.permitted_deciders));
    }
    policy
        .work_units
        .sort_by(|a, b| a.work_unit_id.cmp(&b.work_unit_id));
    if duplicate_by(
        policy
            .work_units
            .iter()
            .map(|item| item.work_unit_id.as_str()),
    ) {
        return Err(invalid("Gate policy contains duplicate WorkUnit IDs"));
    }
    match policy.scope_requirement.as_ref() {
        Some(GateScopeRequirement::MergeOperation {
            operation_id,
            version,
            expected_status,
            gate_evaluation_id,
        }) => {
            validate_text("operation_id", operation_id, 128)?;
            validate_text("gate_evaluation_id", gate_evaluation_id, 128)?;
            if *version < 1
                || !matches!(
                    expected_status.as_str(),
                    "running" | "succeeded" | "conflict" | "failed" | "abandoned"
                )
            {
                return Err(invalid("invalid exact merge-operation scope requirement"));
            }
        }
        Some(GateScopeRequirement::LifecycleOperation {
            transition_id,
            from_state,
            to_state,
            from_version,
            to_version,
            cause_kind,
            cause_ref,
            gate_evaluation_id,
        }) => {
            validate_text("transition_id", transition_id, 128)?;
            validate_text("cause_kind", cause_kind, 64)?;
            if !valid_lifecycle_state(from_state)
                || !valid_lifecycle_state(to_state)
                || *from_version < 1
                || *to_version != *from_version + 1
                || cause_ref.as_ref().is_some_and(|value| value.len() > 256)
                || gate_evaluation_id
                    .as_ref()
                    .is_some_and(|value| value.trim().is_empty() || value.len() > 128)
            {
                return Err(invalid(
                    "invalid exact lifecycle-operation scope requirement",
                ));
            }
        }
        None => {}
    }
    Ok(())
}

fn policy_has_requirements(policy: &GatePolicyDocument) -> bool {
    policy.scope_requirement.is_some()
        || policy.review.is_some()
        || !policy.validations.is_empty()
        || !policy.decisions.is_empty()
        || !policy.work_units.is_empty()
}

fn gate_fact_event(event: &DomainEvent) -> bool {
    if event.event_type == "task.lifecycle_changed" && event.entity_type == "task" {
        return true;
    }
    matches!(
        event.entity_type.as_str(),
        "artifact"
            | "execution"
            | "validation_run"
            | "evidence"
            | "decision"
            | "proposal"
            | "work_unit"
            | "work_unit_integration"
            | "task_role"
            | "role_membership"
            | "task_integration_operation"
            | "gate"
    ) && event.event_type != "gate.evaluated"
}

fn valid_lifecycle_state(state: &str) -> bool {
    matches!(
        state,
        "backlog"
            | "ready"
            | "active"
            | "blocked"
            | "ready_to_merge"
            | "merging"
            | "done"
            | "cancelled"
    )
}

fn lifecycle_transition_subject(transition: &TaskLifecycleTransitionFact) -> Value {
    json!({
        "transition_id": transition.id,
        "task_id": transition.task_id,
        "from_state": transition.from_state.to_string(),
        "to_state": transition.to_state.to_string(),
        "from_version": transition.from_version,
        "to_version": transition.to_version,
        "cause_kind": transition.cause_kind,
        "cause_ref": transition.cause_ref,
        "gate_evaluation_id": transition.gate_evaluation_id,
        "reason_kind": transition.reason_kind,
        "reason_ref": transition.reason_ref,
        "domain_event_id": transition.domain_event_id,
        "created_at": transition.created_at,
    })
}

fn gate_event(
    event_type: &str,
    entity_type: &str,
    entity_id: &str,
    task_id: &str,
    causation_id: Option<String>,
    payload: Value,
    created_at: &str,
) -> CreateDomainEvent {
    CreateDomainEvent {
        id: new_uuid_v4(),
        event_type: event_type.to_owned(),
        entity_type: entity_type.to_owned(),
        entity_id: entity_id.to_owned(),
        actor_type: "system".to_owned(),
        actor_id: None,
        scope_type: "task".to_owned(),
        scope_id: task_id.to_owned(),
        correlation_id: entity_id.to_owned(),
        causation_id,
        causation_depth: 1,
        dedupe_key: Some(format!("{event_type}:{entity_id}")),
        payload_json: payload.to_string(),
        created_at: created_at.to_owned(),
    }
}

fn normalized_actor_refs(mut actors: Vec<ActorRef>) -> Vec<ActorRef> {
    actors.sort_by_key(actor_key);
    actors.dedup();
    actors
}

fn actor_key(actor: &ActorRef) -> String {
    format!("{}:{}", actor.kind(), actor.id())
}

#[derive(Default)]
struct PassingReviewers {
    all: BTreeSet<String>,
    humans: BTreeSet<String>,
}

impl PassingReviewers {
    fn insert(&mut self, actor: &ActorRef) {
        let key = actor_key(actor);
        self.all.insert(key.clone());
        if matches!(actor, ActorRef::Human(_)) {
            self.humans.insert(key);
        }
    }

    fn count(&self) -> usize {
        self.all.len()
    }

    fn has_human(&self) -> bool {
        !self.humans.is_empty()
    }
}

fn actor_set_digest(actors: &[ActorRef]) -> String {
    sha256(
        serde_json::to_string(&normalized_actor_refs(actors.to_vec()))
            .unwrap_or_default()
            .as_bytes(),
    )
}

fn task_role_snapshot_matches(
    snapshot: &TaskRoleReviewerSnapshot,
    task_id: &str,
    role: &db::TaskRole,
    active_members: &[ActorRef],
) -> bool {
    role.id == snapshot.task_role_id
        && role.task_id == task_id
        && role.role == snapshot.role
        && role.version == snapshot.version
        && active_members == snapshot.actor_refs
        && actor_set_digest(&snapshot.actor_refs) == snapshot.membership_digest
}

fn duplicate_by<'a>(values: impl Iterator<Item = &'a str>) -> bool {
    let mut seen = BTreeSet::new();
    values.into_iter().any(|value| !seen.insert(value))
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn validate_text(name: &str, value: &str, max: usize) -> Result<()> {
    if value.trim().is_empty() || value.len() > max {
        return Err(invalid(format!(
            "{name} must contain between 1 and {max} bytes"
        )));
    }
    Ok(())
}

fn invalid(message: impl Into<String>) -> ServiceError {
    ServiceError::invalid_operation(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_role_reviewer_snapshot_is_rechecked_against_membership_fence() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool");
        db::run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(SqliteDb::new(pool));
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let user_id = new_uuid_v4();
        let role_id = new_uuid_v4();
        let membership_id = new_uuid_v4();
        db::UserRepo::create_user(
            &*db,
            &db::User {
                id: user_id.clone(),
                email: "gate-reviewer@example.invalid".to_owned(),
                password_hash: "unused".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human reviewer");
        db::ProjectRepo::create(
            &*db,
            db::CreateProject {
                id: project_id.clone(),
                name: "TaskRole Gate test".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(user_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project");
        db::TaskRepo::create(
            &*db,
            db::CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "TaskRole snapshot".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");
        db::TaskRoleRepo::create(
            &*db,
            db::CreateTaskRole {
                id: role_id.clone(),
                task_id: task_id.clone(),
                role: "reviewer".to_owned(),
                coordination_mode: None,
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("TaskRole");
        db::RoleMembershipRepo::add(
            &*db,
            db::CreateRoleMembership {
                id: membership_id.clone(),
                task_role_id: role_id.clone(),
                actor_kind: ActorKind::Human,
                actor_id: user_id.clone(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("active reviewer membership");
        let role = db::TaskRoleRepo::get_by_id(&*db, &role_id)
            .await
            .expect("TaskRole lookup")
            .expect("TaskRole exists");
        assert_eq!(role.version, 2, "membership write advances the role fence");
        let membership_created_payload: String = sqlx::query_scalar(
            "SELECT payload_json FROM domain_event
             WHERE event_type = 'gate.task_role_changed'
               AND entity_type = 'role_membership' AND entity_id = ?
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(&membership_id)
        .fetch_one(db.pool())
        .await
        .expect("membership creation event");
        assert_eq!(
            serde_json::from_str::<Value>(&membership_created_payload)
                .expect("membership payload")
                .get("task_role_version")
                .and_then(Value::as_i64),
            Some(role.version),
            "membership event must name the exact post-write TaskRole revision"
        );
        let unfenced_policy_update =
            sqlx::query("UPDATE task_role SET policy_json = '{\"reviewers\":[]}' WHERE id = ?")
                .bind(&role_id)
                .execute(db.pool())
                .await;
        assert!(
            unfenced_policy_update.is_err(),
            "TaskRole policy changes must advance the version fence"
        );
        let actor = ActorRef::Human(user_id.clone());
        let snapshot = TaskRoleReviewerSnapshot {
            task_role_id: role_id.clone(),
            role: "reviewer".to_owned(),
            version: role.version,
            membership_digest: actor_set_digest(std::slice::from_ref(&actor)),
            actor_refs: vec![actor.clone()],
        };
        let execution_id = new_uuid_v4();
        db::ExecutionRepo::create(
            &*db,
            db::CreateExecution {
                id: execution_id.clone(),
                task_id: task_id.clone(),
                agent_id: None,
                actor_ref: Some(actor.clone()),
                role: "reviewer".to_owned(),
                purpose: Some(ExecutionPurpose::Review),
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
        .expect("review Execution");
        let report_id = new_uuid_v4();
        let report_content = json!({
            "kind": "review_report",
            "verdict": "pass",
            "summary": "exact Human report",
            "criteria": ["required check"],
            "findings": [],
            "questions": [],
            "evidence_considered": [],
            "subject": {
                "task_id": task_id,
                "review_execution_id": execution_id,
                "workspace_id": null,
                "base_commit_sha": null,
                "head_commit_sha": null,
                "workspace_snapshot_digest": null,
            }
        })
        .to_string();
        let report_digest = sha256(report_content.as_bytes());
        db::CollaborationRepo::create_execution_artifact_output(
            &*db,
            db::CreateArtifact {
                id: report_id.clone(),
                task_id: task_id.clone(),
                kind: ArtifactKind::ReviewReport,
                storage_kind: db::ArtifactStorageKind::Inline,
                content: Some(report_content),
                content_ref: None,
                metadata_json: json!({ "schema_version": 1 }).to_string(),
                digest: Some(report_digest.clone()),
                producer_execution_id: execution_id.clone(),
                created_at: now.clone(),
            },
            gate_event(
                "artifact.created",
                "artifact",
                &report_id,
                &task_id,
                None,
                json!({ "kind": "review_report", "execution_id": execution_id }),
                &now,
            ),
        )
        .await
        .expect("exact ReviewReport output");
        let mut passing_report = candidate(&report_id);
        passing_report.digest = report_digest;
        passing_report.expected_actor = Some(actor);
        let exact_candidate = passing_report.clone();
        let exact_role_snapshot = snapshot.clone();
        let policy = GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: Some(ReviewSetPolicy {
                mode: ReviewSelectionMode::OneAcceptable,
                required_count: 1,
                human_required: true,
                allow_humans: true,
                allow_agents: false,
                allowed_actor_refs: Vec::new(),
                task_role_snapshot: Some(snapshot),
                candidates: vec![candidate("optional-not-yet-produced"), passing_report],
            }),
            validations: Vec::new(),
            decisions: Vec::new(),
            work_units: Vec::new(),
        };
        let event_bus = Arc::new(events::EventBus::new(16));
        let engine = GateEngine::new(Arc::clone(&db), event_bus);
        let (gate, _) = engine
            .create_gate_with_initial_policy(
                &task_id,
                "merge_readiness",
                GateScopeKind::Task,
                &task_id,
                policy,
            )
            .await
            .expect("valid exact reviewer snapshot");
        let while_running = engine
            .evaluate_active(&gate.id)
            .await
            .expect("running Review Execution is not acceptable");
        assert_eq!(
            while_running.evaluation.outcome,
            GateEvaluationOutcome::Unsatisfied
        );

        db::ExecutionRepo::update(
            &*db,
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
        .expect("review completes after its exact report is durable");
        let completed_event = db::DomainEventRepo::append_event(
            &*db,
            gate_event(
                "execution.completed",
                "execution",
                &execution_id,
                &task_id,
                None,
                json!({"execution_id": execution_id, "task_id": task_id}),
                &now_rfc3339(),
            ),
        )
        .await
        .expect("durable Review Execution completion event");
        assert_eq!(
            engine
                .process_domain_event(&completed_event)
                .await
                .expect("Execution completion reevaluates the exact ReviewReport"),
            1
        );
        let first = engine
            .evaluate_revision(gate.clone(), 1)
            .await
            .expect("completed Review Execution satisfies the reviewer candidate");
        assert!(
            db::GateRepo::is_latest_gate_evaluation(&*db, &gate.id, 1, &first.evaluation.id,)
                .await
                .expect("initial exact evaluation is current")
        );
        assert!(first
            .inputs
            .iter()
            .any(|input| input.input_kind == "task_role_snapshot" && input.status == "current"));
        assert_eq!(first.evaluation.outcome, GateEvaluationOutcome::Satisfied);
        let first_result: GateEvaluationResult =
            serde_json::from_str(&first.evaluation.result_json).expect("evaluation result");
        assert!(first_result.missing_inputs.is_empty());

        let reviewer_policy = |mode,
                               required_count,
                               mut exact_candidate: ReviewReportRequirement,
                               missing_required| {
            exact_candidate.required = missing_required;
            let mut missing_candidate = candidate("optional-not-yet-produced");
            missing_candidate.required = missing_required;
            GatePolicyDocument {
                schema_version: 1,
                scope_requirement: None,
                review: Some(ReviewSetPolicy {
                    mode,
                    required_count,
                    human_required: true,
                    allow_humans: true,
                    allow_agents: false,
                    allowed_actor_refs: Vec::new(),
                    task_role_snapshot: Some(exact_role_snapshot.clone()),
                    candidates: vec![exact_candidate, missing_candidate],
                }),
                validations: Vec::new(),
                decisions: Vec::new(),
                work_units: Vec::new(),
            }
        };
        let mut wrong_reviewer = exact_candidate.clone();
        wrong_reviewer.expected_actor = Some(ActorRef::Human("different-reviewer".to_owned()));
        engine
            .revise_policy(
                &gate.id,
                Some(1),
                reviewer_policy(ReviewSelectionMode::OneAcceptable, 1, wrong_reviewer, false),
            )
            .await
            .expect("wrong-reviewer policy revision");
        let wrong_reviewer_evaluation = engine
            .evaluate_active(&gate.id)
            .await
            .expect("wrong reviewer stays unsatisfied");
        assert_eq!(
            wrong_reviewer_evaluation.evaluation.outcome,
            GateEvaluationOutcome::Unsatisfied
        );

        engine
            .revise_policy(
                &gate.id,
                Some(2),
                reviewer_policy(
                    ReviewSelectionMode::AtLeast,
                    1,
                    exact_candidate.clone(),
                    false,
                ),
            )
            .await
            .expect("N-of-M reviewer policy revision");
        let quorum = engine
            .evaluate_active(&gate.id)
            .await
            .expect("one of two reviewer candidates satisfies N-of-M");
        assert_eq!(quorum.evaluation.outcome, GateEvaluationOutcome::Satisfied);

        engine
            .revise_policy(
                &gate.id,
                Some(3),
                reviewer_policy(ReviewSelectionMode::AllRequired, 2, exact_candidate, true),
            )
            .await
            .expect("all-reviewers policy revision");
        let all_reviewers = engine
            .evaluate_active(&gate.id)
            .await
            .expect("missing required reviewer does not hide the passing reviewer");
        assert_eq!(
            all_reviewers.evaluation.outcome,
            GateEvaluationOutcome::Unsatisfied
        );

        db::RoleMembershipRepo::update(
            &*db,
            db::UpdateRoleMembership {
                id: membership_id.clone(),
                expected_version: 1,
                status: RoleMembershipStatus::Ended,
                updated_at: now_rfc3339(),
                ended_at: Some(now_rfc3339()),
            },
        )
        .await
        .expect("reviewer membership ends");
        assert_eq!(
            engine
                .process_domain_event(&completed_event)
                .await
                .expect("replaying the source event keeps its original evaluation"),
            0
        );
        let evaluation_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM gate_evaluation WHERE gate_id = ?")
                .bind(&gate.id)
                .fetch_one(db.pool())
                .await
                .expect("evaluation count");
        assert_eq!(evaluation_count, 5, "source replay must not create E2");
        let first_event_id: String = sqlx::query_scalar(
            "SELECT id FROM domain_event
             WHERE event_type = 'gate.evaluated' AND entity_id = ?",
        )
        .bind(&first.evaluation.id)
        .fetch_one(db.pool())
        .await
        .expect("the exact evaluation event remains durable");
        let first_event = db::DomainEventRepo::get_event(&*db, &first_event_id)
            .await
            .expect("evaluation event lookup")
            .expect("evaluation event");
        engine
            .process_domain_event(&first_event)
            .await
            .expect("stale E1 replay is safely consumed without lifecycle movement");
        assert_eq!(
            TaskLifecycleRepo::get_task_lifecycle(&*db, &task_id)
                .await
                .expect("lifecycle lookup")
                .expect("lifecycle")
                .state,
            TaskLifecycleState::Ready,
            "E1 must not authorize readiness after its TaskRole input changed"
        );

        let membership_event_id: String = sqlx::query_scalar(
            "SELECT id FROM domain_event
             WHERE event_type = 'gate.task_role_changed'
               AND entity_type = 'role_membership' AND entity_id = ?
             ORDER BY sequence DESC LIMIT 1",
        )
        .bind(&membership_id)
        .fetch_one(db.pool())
        .await
        .expect("membership change event");
        let membership_event = db::DomainEventRepo::get_event(&*db, &membership_event_id)
            .await
            .expect("membership event lookup")
            .expect("membership event");
        let changed_role = db::TaskRoleRepo::get_by_id(&*db, &role_id)
            .await
            .expect("TaskRole lookup after membership change")
            .expect("TaskRole exists");
        assert_eq!(
            serde_json::from_str::<Value>(&membership_event.payload_json)
                .expect("membership payload")
                .get("task_role_version")
                .and_then(Value::as_i64),
            Some(changed_role.version),
            "membership event must match the current post-write TaskRole revision"
        );
        assert_eq!(
            engine
                .process_domain_event(&membership_event)
                .await
                .expect("membership change reevaluates the reviewer policy"),
            1
        );
        let second = GateRepo::get_gate_evaluation_for_cause(&*db, &gate.id, &membership_event.id)
            .await
            .expect("causal E2 lookup")
            .expect("membership change evaluation");
        assert!(db::GateRepo::is_latest_gate_evaluation(
            &*db,
            &first.evaluation.gate_id,
            1,
            &first.evaluation.id,
        )
        .await
        .expect("E1 remains the latest immutable evaluation for its original policy revision"));
        assert_eq!(second.policy_revision, 4);
        assert!(db::GateRepo::is_latest_gate_evaluation(
            &*db,
            &second.gate_id,
            second.policy_revision,
            &second.id,
        )
        .await
        .expect("new exact evaluation is current"));
        let result: GateEvaluationResult =
            serde_json::from_str(&second.result_json).expect("evaluation result");
        assert!(result
            .issues
            .iter()
            .any(|issue| issue.starts_with("task_role_snapshot_stale:")));
        assert!(second.outcome.eq(&GateEvaluationOutcome::Unsatisfied));
        assert!(
            !GateRepo::gate_evaluation_inputs_are_current(&*db, &first.evaluation.id)
                .await
                .expect("TaskRole fence invalidates E1")
        );
        assert_eq!(second.outcome, GateEvaluationOutcome::Unsatisfied);

        let task_role = db::TaskRoleRepo::get_by_id(&*db, &role_id)
            .await
            .expect("TaskRole lookup")
            .expect("TaskRole exists");
        db::TaskRoleRepo::update(
            &*db,
            db::UpdateTaskRole {
                id: role_id.clone(),
                expected_version: task_role.version,
                coordination_mode: None,
                policy_json: Some(json!({"review_policy_revision": 2}).to_string()),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("TaskRole policy revision");
        let role_policy_event_id: String = sqlx::query_scalar(
            "SELECT id FROM domain_event
             WHERE event_type = 'gate.task_role_changed'
               AND entity_type = 'task_role' AND entity_id = ?",
        )
        .bind(&role_id)
        .fetch_one(db.pool())
        .await
        .expect("TaskRole revision event");
        let role_policy_event = db::DomainEventRepo::get_event(&*db, &role_policy_event_id)
            .await
            .expect("TaskRole event lookup")
            .expect("TaskRole revision event");
        assert_eq!(
            engine
                .process_domain_event(&role_policy_event)
                .await
                .expect("TaskRole revision reevaluates the frozen reviewer set"),
            1
        );
        let membership_events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM domain_event
             WHERE event_type = 'gate.task_role_changed' AND scope_id = ?",
        )
        .bind(&task_id)
        .fetch_one(db.pool())
        .await
        .expect("TaskRole membership events");
        assert_eq!(membership_events, 3);
    }

    #[tokio::test]
    async fn work_unit_gate_uses_admission_version_and_requires_exact_integration() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool");
        db::run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(SqliteDb::new(pool));
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let human_id = new_uuid_v4();
        let role_id = new_uuid_v4();
        let membership_id = new_uuid_v4();
        let unit_id = new_uuid_v4();
        let execution_id = new_uuid_v4();
        let result_sha = "a".repeat(40);

        db::UserRepo::create_user(
            &*db,
            &db::User {
                id: human_id.clone(),
                email: "work-unit-gate@example.invalid".to_owned(),
                password_hash: "unused".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human Actor");
        db::ProjectRepo::create(
            &*db,
            db::CreateProject {
                id: project_id.clone(),
                name: "WorkUnit Gate test".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(human_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project");
        db::TaskRepo::create(
            &*db,
            db::CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Exact WorkUnit Gate".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task");

        let actor = ActorRef::Human(human_id);
        db::TaskRoleRepo::create(
            &*db,
            db::CreateTaskRole {
                id: role_id.clone(),
                task_id: task_id.clone(),
                role: "implementer".to_owned(),
                coordination_mode: None,
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("implementer TaskRole");
        db::RoleMembershipRepo::add(
            &*db,
            db::CreateRoleMembership {
                id: membership_id,
                task_role_id: role_id,
                actor_kind: ActorKind::Human,
                actor_id: actor.id().to_owned(),
                status: RoleMembershipStatus::Active,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("active implementer membership");
        WorkUnitRepo::create(
            &*db,
            db::CreateWorkUnit {
                id: unit_id.clone(),
                task_id: task_id.clone(),
                parent_work_unit_id: None,
                title: "Implement exact unit".to_owned(),
                scope: "One bounded change".to_owned(),
                role: "implementer".to_owned(),
                assigned_actor: Some(actor.clone()),
                requires_integration: false,
                provenance: None,
                created_by: actor.clone(),
                created_at: now.clone(),
            },
            gate_event(
                "work_unit.created",
                "work_unit",
                &unit_id,
                &task_id,
                None,
                json!({"version": 1, "status": "open"}),
                &now,
            ),
        )
        .await
        .expect("WorkUnit");
        db::WorkUnitExecutionRepo::create_for_work_unit(
            &*db,
            db::CreateWorkUnitExecution {
                execution: db::CreateExecution {
                    id: execution_id.clone(),
                    task_id: task_id.clone(),
                    agent_id: None,
                    actor_ref: Some(actor),
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
                work_unit_id: unit_id.clone(),
                work_unit_version: 1,
                workspace_lease: None,
            },
            gate_event(
                "execution.started",
                "execution",
                &execution_id,
                &task_id,
                None,
                json!({"work_unit_id": unit_id, "work_unit_version": 1}),
                &now,
            ),
        )
        .await
        .expect("Execution admitted against WorkUnit version 1");
        db::ExecutionRepo::update(
            &*db,
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
                after_sha: Some(Some(result_sha.clone())),
                error: None,
                executor_config_snapshot_json: None,
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Execution completes with exact result");
        let completed_unit = WorkUnitRepo::transition(
            &*db,
            db::TransitionWorkUnit {
                id: unit_id.clone(),
                expected_version: 1,
                status: WorkUnitStatus::Completed,
                updated_at: now.clone(),
            },
            gate_event(
                "work_unit.completed",
                "work_unit",
                &unit_id,
                &task_id,
                None,
                json!({"version": 2, "status": "completed"}),
                &now,
            ),
        )
        .await
        .expect("WorkUnit completion advances its version")
        .record;
        assert_eq!(completed_unit.version, 2);

        let requirement = WorkUnitRequirement {
            work_unit_id: unit_id.clone(),
            version: completed_unit.version,
            dependency_digest: sha256(b"[]"),
            execution_id,
            execution_result_sha: result_sha,
            require_integration: false,
            integration_id: None,
            integration_version: None,
        };
        let policy = |require_integration| GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: Vec::new(),
            decisions: Vec::new(),
            work_units: vec![WorkUnitRequirement {
                require_integration,
                ..requirement.clone()
            }],
        };
        let event_bus = Arc::new(events::EventBus::new(16));
        let engine = GateEngine::new(Arc::clone(&db), event_bus);
        let (gate, _) = engine
            .create_gate_with_initial_policy(
                &task_id,
                "work_unit_completion",
                GateScopeKind::WorkUnit,
                &unit_id,
                policy(false),
            )
            .await
            .expect("exact WorkUnit-scoped policy");
        let satisfied = engine
            .evaluate_active(&gate.id)
            .await
            .expect("completion with exact admitted Execution satisfies Gate");
        assert_eq!(
            satisfied.evaluation.outcome,
            GateEvaluationOutcome::Satisfied
        );
        assert!(satisfied
            .inputs
            .iter()
            .any(|input| input.input_kind == "execution" && input.input_version == 1));
        assert!(satisfied
            .inputs
            .iter()
            .any(|input| input.input_kind == "work_unit" && input.input_version == 2));
        assert!(
            db::GateRepo::gate_evaluation_inputs_are_current(&*db, &satisfied.evaluation.id)
                .await
                .expect("exact WorkUnit and Execution facts remain current")
        );

        engine
            .revise_policy(&gate.id, Some(1), policy(true))
            .await
            .expect("integration-required revision");
        let incomplete = engine
            .evaluate_active(&gate.id)
            .await
            .expect("completion alone cannot satisfy integration requirement");
        assert_eq!(
            incomplete.evaluation.outcome,
            GateEvaluationOutcome::Unsatisfied
        );
        let result: GateEvaluationResult =
            serde_json::from_str(&incomplete.evaluation.result_json).expect("result JSON");
        assert!(result
            .issues
            .iter()
            .any(|issue| issue == &format!("work_unit_integration_required:{unit_id}")));

        sqlx::query("UPDATE execution SET after_sha = ?, updated_at = ? WHERE id = ?")
            .bind("f".repeat(40))
            .bind(now_rfc3339())
            .bind(&requirement.execution_id)
            .execute(db.pool())
            .await
            .expect("simulate mutation of the exact completed Execution result");
        assert!(
            !db::GateRepo::gate_evaluation_inputs_are_current(&*db, &satisfied.evaluation.id)
                .await
                .expect("stale exact WorkUnit result is detected")
        );
    }

    #[tokio::test]
    async fn decision_gate_requires_exact_proposal_policy_outcome_and_decider() {
        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool");
        db::run_migrations(&pool).await.expect("migrations");
        let db = Arc::new(SqliteDb::new(pool));
        let now = now_rfc3339();
        let project_id = new_uuid_v4();
        let task_id = new_uuid_v4();
        let human_id = new_uuid_v4();
        let policy_ref = "forge.merge-approval".to_owned();
        let policy_version = 3;
        let policy_digest = "b".repeat(64);

        db::UserRepo::create_user(
            &*db,
            &db::User {
                id: human_id.clone(),
                email: "decision-gate@example.invalid".to_owned(),
                password_hash: "unused".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Human Actor");
        db::ProjectRepo::create(
            &*db,
            db::CreateProject {
                id: project_id.clone(),
                name: "Decision Gate test".to_owned(),
                settings: "{}".to_owned(),
                workflow_definition: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(human_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project");
        db::TaskRepo::create(
            &*db,
            db::CreateTask {
                id: task_id.clone(),
                project_id,
                repo_id: None,
                parent_task_id: None,
                assignee_type: None,
                assignee_id: None,
                title: "Exact Decision Gate".to_owned(),
                description: None,
                task_type: "implementation".to_owned(),
                status: "todo".to_owned(),
                is_automation: false,
                priority: 0,
                subtask_order: None,
                task_state_config: None,
                merge_config: None,
                created_at: now,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .expect("Task");

        let event_bus = Arc::new(events::EventBus::new(16));
        let collaboration = crate::collaboration_service::CollaborationService::new(
            Arc::clone(&db),
            Arc::clone(&event_bus),
        );
        let proposal = collaboration
            .create_proposal(
                crate::collaboration_service::CollaborationActorSource::Human(human_id.clone()),
                crate::collaboration_service::CreateProposalInput {
                    task_id: task_id.clone(),
                    target: db::ProposalTarget {
                        kind: db::ProposalTargetKind::Task,
                        id: task_id.clone(),
                    },
                    action: "merge".to_owned(),
                    reason: "Merge after deterministic readiness".to_owned(),
                    target_version: Some(1),
                    target_digest: Some("c".repeat(64)),
                    required_policy_ref: Some(policy_ref.clone()),
                    required_policy_version: Some(policy_version),
                    required_policy_digest: Some(policy_digest.clone()),
                    supersedes_proposal_id: None,
                    artifact_ids: Vec::new(),
                },
            )
            .await
            .expect("Proposal");
        let decision = collaboration
            .record_decision(
                crate::collaboration_service::CreateDecisionInput {
                    task_id: task_id.clone(),
                    proposal_id: proposal.id.clone(),
                    proposal_version: proposal.content_version,
                    outcome: DecisionOutcome::Approve,
                    rationale: "Exact Human approval".to_owned(),
                    policy_ref: Some(policy_ref.clone()),
                    policy_version: Some(policy_version),
                    policy_digest: Some(policy_digest.clone()),
                },
                vec![
                    crate::collaboration_service::CollaborationActorSource::Human(human_id.clone()),
                ],
            )
            .await
            .expect("exact Human Decision");

        let requirement = DecisionRequirement {
            proposal_id: proposal.id.clone(),
            proposal_version: proposal.content_version,
            decision_id: decision.id.clone(),
            outcome: DecisionOutcome::Approve,
            policy_ref: Some(policy_ref.clone()),
            policy_version: Some(policy_version),
            policy_digest: Some(policy_digest.clone()),
            permitted_deciders: vec![ActorRef::Human(human_id.clone())],
        };
        let policy = |requirement| GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: None,
            validations: Vec::new(),
            decisions: vec![requirement],
            work_units: Vec::new(),
        };
        let engine = GateEngine::new(Arc::clone(&db), event_bus);
        let (gate, _) = engine
            .create_gate_with_initial_policy(
                &task_id,
                "human_approval",
                GateScopeKind::Task,
                &task_id,
                policy(requirement.clone()),
            )
            .await
            .expect("exact Decision policy");
        let exact = engine
            .evaluate_active(&gate.id)
            .await
            .expect("exact Proposal and Human approval satisfy Gate");
        assert_eq!(exact.evaluation.outcome, GateEvaluationOutcome::Satisfied);
        assert_eq!(
            exact
                .inputs
                .iter()
                .find(|input| input.input_kind == "decision")
                .and_then(|input| input.producer_ref.as_deref()),
            Some(proposal.id.as_str())
        );

        let mut mismatches = Vec::new();
        let mut wrong_version = requirement.clone();
        wrong_version.proposal_version += 1;
        mismatches.push(wrong_version);
        let mut wrong_outcome = requirement.clone();
        wrong_outcome.outcome = DecisionOutcome::Reject;
        mismatches.push(wrong_outcome);
        let mut wrong_policy = requirement.clone();
        wrong_policy.policy_digest = Some("d".repeat(64));
        mismatches.push(wrong_policy);
        let mut wrong_decider = requirement;
        wrong_decider.permitted_deciders = vec![ActorRef::Human("different-human".to_owned())];
        mismatches.push(wrong_decider);

        let mut active_revision = 1;
        for mismatch in mismatches {
            engine
                .revise_policy(&gate.id, Some(active_revision), policy(mismatch))
                .await
                .expect("append immutable test policy revision");
            active_revision += 1;
            let evaluation = engine
                .evaluate_active(&gate.id)
                .await
                .expect("mismatched authorization stays unsatisfied");
            assert_eq!(
                evaluation.evaluation.outcome,
                GateEvaluationOutcome::Unsatisfied
            );
            let result: GateEvaluationResult =
                serde_json::from_str(&evaluation.evaluation.result_json).expect("result JSON");
            assert!(result
                .issues
                .iter()
                .any(|issue| issue.starts_with("decision_authority_mismatch:")));
        }
    }

    fn candidate(id: &str) -> ReviewReportRequirement {
        ReviewReportRequirement {
            artifact_id: id.to_owned(),
            digest: "a".repeat(64),
            expected_actor: None,
            required: false,
            subject: ReviewSubjectExpectation {
                workspace_id: None,
                base_commit_sha: None,
                head_commit_sha: None,
                workspace_snapshot_digest: None,
            },
        }
    }

    fn review_policy(mode: ReviewSelectionMode, required_count: u32) -> GatePolicyDocument {
        GatePolicyDocument {
            schema_version: 1,
            scope_requirement: None,
            review: Some(ReviewSetPolicy {
                mode,
                required_count,
                human_required: false,
                allow_humans: true,
                allow_agents: true,
                allowed_actor_refs: Vec::new(),
                task_role_snapshot: None,
                candidates: vec![
                    candidate("review-c"),
                    candidate("review-a"),
                    candidate("review-b"),
                ],
            }),
            validations: Vec::new(),
            decisions: Vec::new(),
            work_units: Vec::new(),
        }
    }

    #[test]
    fn review_policy_quorums_are_bounded_and_normalized() {
        let mut one = review_policy(ReviewSelectionMode::OneAcceptable, 1);
        normalize_policy(&mut one).expect("one acceptable reviewer");
        assert_eq!(
            one.review
                .as_ref()
                .unwrap()
                .candidates
                .iter()
                .map(|candidate| candidate.artifact_id.as_str())
                .collect::<Vec<_>>(),
            vec!["review-a", "review-b", "review-c"]
        );

        let mut all = review_policy(ReviewSelectionMode::AllRequired, 3);
        for candidate in &mut all.review.as_mut().unwrap().candidates {
            candidate.required = true;
        }
        normalize_policy(&mut all).expect("all required reviewers");
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AllRequired, 2)).is_err());
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AllRequired, 3)).is_err());
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AtLeast, 0)).is_err());
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AtLeast, 4)).is_err());
        assert!(
            normalize_policy(&mut review_policy(ReviewSelectionMode::OneAcceptable, 2)).is_err()
        );

        let mut duplicate = review_policy(ReviewSelectionMode::OneAcceptable, 1);
        duplicate.review.as_mut().unwrap().candidates[2] = candidate("review-a");
        assert!(normalize_policy(&mut duplicate).is_err());
    }

    #[test]
    fn review_quorum_counts_distinct_actor_refs_not_report_rows() {
        let reviewer = ActorRef::Agent("reviewer-a".to_owned());
        let mut same_reviewer_twice = PassingReviewers::default();
        same_reviewer_twice.insert(&reviewer);
        same_reviewer_twice.insert(&reviewer);
        assert_eq!(same_reviewer_twice.count(), 1);
        assert!(!same_reviewer_twice.has_human());

        let human = ActorRef::Human("reviewer-human".to_owned());
        same_reviewer_twice.insert(&human);
        assert_eq!(same_reviewer_twice.count(), 2);
        assert!(same_reviewer_twice.has_human());
    }
}
