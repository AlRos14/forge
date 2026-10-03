use std::{collections::BTreeSet, sync::Arc};

use db::{
    new_uuid_v4, now_rfc3339, ActorKind, ActorRef, ArtifactKind, CollaborationRepo,
    CreateDomainEvent, CreateGate, CreateGatePolicyRevision, DecisionOutcome, DomainEvent,
    ExecutionPurpose, ExecutionRepo, ExecutionStatus, Gate, GateEvaluation, GateEvaluationInput,
    GateEvaluationOutcome, GateEvaluationWrite, GatePolicyRevision, GateRepo, GateScopeKind,
    ProposalStatus, RoleMembershipRepo, RoleMembershipStatus, SqliteDb, TaskLifecycleRepo,
    TaskLifecycleState, TaskRepo, TaskRoleRepo, ValidationRunRepo, ValidationRunStatus,
    WorkUnitRepo, WorkUnitStatus,
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
    pub review: Option<ReviewSetPolicy>,
    #[serde(default)]
    pub validations: Vec<ValidationRequirement>,
    #[serde(default)]
    pub decisions: Vec<DecisionRequirement>,
    #[serde(default)]
    pub work_units: Vec<WorkUnitRequirement>,
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
            let write = self
                .evaluate_revision_with_cause(gate.clone(), policy.revision, Some(event.id.clone()))
                .await?;
            if write.event.is_some() {
                evaluated += 1;
            }
            self.apply_merge_readiness_evaluation(&gate, &write.evaluation)
                .await?;
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
            TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
                .transition(TransitionLifecycleInput {
                    task_id: task.id.clone(),
                    expected_task_version: task.version,
                    to_state: TaskLifecycleState::ReadyToMerge,
                    cause: LifecycleCause::GateEvaluation(evaluation.id.clone()),
                    reason_kind: Some("merge_readiness_satisfied".to_owned()),
                    reason_ref: Some(evaluation.id.clone()),
                    idempotency_key: format!("gate-evaluation:{}:ready-to-merge", evaluation.id),
                })
                .await?;
        }
        Ok(())
    }

    async fn validate_policy_scope(&self, gate: &Gate, policy: &GatePolicyDocument) -> Result<()> {
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
        let mut passing = 0_u32;
        let mut passing_humans = 0_u32;
        let mut required_failed = false;
        for candidate in &requirement.candidates {
            let Some(artifact) =
                CollaborationRepo::get_artifact(&*self.db, &candidate.artifact_id).await?
            else {
                missing.push(format!("review_report:{}", candidate.artifact_id));
                if candidate.required {
                    required_failed = true;
                }
                continue;
            };
            let Some((execution_id, producer_actor)) = artifact.execution_producer() else {
                issues.push(format!("invalid_review_producer:{}", candidate.artifact_id));
                if candidate.required {
                    required_failed = true;
                }
                continue;
            };
            if artifact.task_id != gate.task_id
                || artifact.kind != ArtifactKind::ReviewReport
                || artifact.digest.as_deref() != Some(candidate.digest.as_str())
                || artifact.content_ref.is_some()
            {
                issues.push(format!(
                    "review_report_identity_mismatch:{}",
                    candidate.artifact_id
                ));
                if candidate.required {
                    required_failed = true;
                }
                continue;
            }
            let Some(execution) = ExecutionRepo::get_by_id(&*self.db, execution_id).await? else {
                issues.push(format!(
                    "review_execution_missing:{}",
                    candidate.artifact_id
                ));
                if candidate.required {
                    required_failed = true;
                }
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
            let task_role_ok = requirement
                .task_role_snapshot
                .as_ref()
                .is_none_or(|snapshot| snapshot.actor_refs.contains(producer_actor));
            let producer_ok = execution.task_id == gate.task_id
                && execution.role == "reviewer"
                && execution.purpose == Some(ExecutionPurpose::Review)
                && execution.status == ExecutionStatus::Completed
                && actor_kind_ok
                && actor_ok
                && task_role_ok
                && subject_matches
                && matches!(verdict, "pass" | "request_changes" | "questions");
            if !producer_ok {
                issues.push(format!(
                    "review_report_not_acceptable:{}",
                    candidate.artifact_id
                ));
                if candidate.required {
                    required_failed = true;
                }
            } else if verdict == "pass" {
                passing += 1;
                if matches!(producer_actor, ActorRef::Human(_)) {
                    passing_humans += 1;
                }
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
        if let Some(snapshot) = &requirement.task_role_snapshot {
            let members = normalized_actor_refs(snapshot.actor_refs.clone());
            let subject_json = json!({
                "task_role_id": snapshot.task_role_id,
                "role": snapshot.role,
                "members": members,
            });
            inputs.push(InputDraft {
                input_kind: "task_role_snapshot".to_owned(),
                input_id: snapshot.task_role_id.clone(),
                input_version: snapshot.version,
                input_digest: snapshot.membership_digest.clone(),
                producer_ref: None,
                subject_json: subject_json.to_string(),
                status: "frozen".to_owned(),
            });
        }
        let threshold = match requirement.mode {
            ReviewSelectionMode::OneAcceptable => 1,
            ReviewSelectionMode::AllRequired => requirement.candidates.len() as u32,
            ReviewSelectionMode::AtLeast => requirement.required_count,
        };
        if passing < threshold {
            issues.push("review_quorum_not_satisfied".to_owned());
        }
        if requirement.human_required && passing_humans == 0 {
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
            let execution_exact = execution.task_id == gate.task_id
                && execution.work_unit_id.as_deref() == Some(unit.id.as_str())
                && execution.work_unit_version == Some(requirement.version)
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
                        "workspace_id": integration.target_workspace_id,
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
    Ok(())
}

fn policy_has_requirements(policy: &GatePolicyDocument) -> bool {
    policy.review.is_some()
        || !policy.validations.is_empty()
        || !policy.decisions.is_empty()
        || !policy.work_units.is_empty()
}

fn gate_fact_event(event: &DomainEvent) -> bool {
    matches!(
        event.entity_type.as_str(),
        "artifact"
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

fn actor_set_digest(actors: &[ActorRef]) -> String {
    sha256(
        serde_json::to_string(&normalized_actor_refs(actors.to_vec()))
            .unwrap_or_default()
            .as_bytes(),
    )
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
        normalize_policy(&mut all).expect("all required reviewers");
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AllRequired, 2)).is_err());
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AtLeast, 0)).is_err());
        assert!(normalize_policy(&mut review_policy(ReviewSelectionMode::AtLeast, 4)).is_err());
        assert!(
            normalize_policy(&mut review_policy(ReviewSelectionMode::OneAcceptable, 2)).is_err()
        );

        let mut duplicate = review_policy(ReviewSelectionMode::OneAcceptable, 1);
        duplicate.review.as_mut().unwrap().candidates[2] = candidate("review-a");
        assert!(normalize_policy(&mut duplicate).is_err());
    }
}
