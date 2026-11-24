use super::*;
use crate::{
    coordinator_outputs::{EvidenceKind, EvidenceLink, ProposeReplanOutput},
    human_decision_service::{HumanDecisionService, RaiseHumanDecisionInput, ResumeBinding},
    replan_classifier::{
        classify_replan, ReplanClassification, ReplanClassificationResult, ReplanOriginalScope,
        ReplanRepositoryMember,
    },
};
use db::{
    CoordinatorReplanClassification, CoordinatorReplanState, CreateCoordinatorReplan,
    DomainEventRepo, HumanDecisionRepo, HumanDecisionSourceKind, TaskCoordinationRequestKind,
    TaskCoordinatorReplanRepo, TransitionCoordinatorReplan, WorkUnitRepo,
};
use std::collections::BTreeMap;

pub(super) struct RaiseReplanScopeDecisionInput<'a> {
    pub(super) request: &'a TaskCoordinationRequest,
    pub(super) execution: &'a db::Execution,
    pub(super) artifact_id: &'a str,
    pub(super) artifact_digest: &'a str,
    pub(super) replan_id: &'a str,
    pub(super) classification: ReplanClassification,
    pub(super) facts_json: String,
}

fn replan_actor_key(actor: &db::ActorRef) -> String {
    match actor {
        db::ActorRef::Human(id) => format!("human:{id}"),
        db::ActorRef::Agent(id) => format!("agent:{id}"),
    }
}

fn map_replan_db_error(error: db::DbError) -> ServiceError {
    if matches!(error, db::DbError::CoordinatorReplanLimitReached) {
        coordination_error(
            "coordination.replan_limit_reached",
            "a Task may receive at most two coordinator replans",
        )
    } else {
        error.into()
    }
}

fn parse_replan_context(
    brief: &CoordinationBrief,
) -> Result<Option<(String, ReplanOriginalScope)>> {
    let Some(context) = brief.context.as_deref() else {
        return Ok(None);
    };
    let value: Value = serde_json::from_str(context)
        .map_err(|_| invalid_plan("stored coordination context is invalid JSON"))?;
    let Some(id) = value.get("coordinator_replan_id").and_then(Value::as_str) else {
        return Ok(None);
    };
    let original: ReplanOriginalScope = serde_json::from_value(
        value
            .get("replan_original_scope")
            .cloned()
            .ok_or_else(|| invalid_plan("replan context has no original scope snapshot"))?,
    )
    .map_err(|_| invalid_plan("replan original scope snapshot is invalid"))?;
    Ok(Some((id.to_owned(), original)))
}

struct ExactReplanDecisionBinding<'a> {
    task_id: &'a str,
    replan_id: &'a str,
    authorization_version: i64,
    request_id: &'a str,
    artifact_id: &'a str,
    plan_digest: &'a str,
    user_id: &'a str,
}

fn validate_exact_replan_decision(
    decision: &db::HumanDecisionRequest,
    expected: ExactReplanDecisionBinding<'_>,
) -> Result<()> {
    if decision.task_id != expected.task_id
        || decision.source_kind != HumanDecisionSourceKind::CoordinatorReplan
        || decision.source_id != expected.replan_id
        || decision.authorization_version != Some(expected.authorization_version)
        || decision.state != db::HumanDecisionState::Answered
        || decision.answered_option_id.as_deref() != Some("approve_replan")
        || decision.answered_by_user_id.as_deref() != Some(expected.user_id)
    {
        return Err(invalid_plan(
            "the exact Human replan decision is not approved",
        ));
    }
    let binding: ResumeBinding = serde_json::from_str(&decision.resume_binding_json)
        .map_err(|_| invalid_plan("the Human decision resume binding is invalid"))?;
    if binding.request_id != expected.request_id
        || binding.request_kind != TaskCoordinationRequestKind::WorkUnitPlan
        || binding.authorization_version != expected.authorization_version
        || binding
            .source_subject
            .get("coordinator_replan_id")
            .and_then(Value::as_str)
            != Some(expected.replan_id)
        || binding
            .source_subject
            .get("plan_request_id")
            .and_then(Value::as_str)
            != Some(expected.request_id)
        || binding
            .source_subject
            .get("plan_artifact_id")
            .and_then(Value::as_str)
            != Some(expected.artifact_id)
        || binding
            .source_subject
            .get("plan_digest")
            .and_then(Value::as_str)
            != Some(expected.plan_digest)
    {
        return Err(invalid_plan(
            "the Human decision does not bind this exact replan artifact",
        ));
    }
    Ok(())
}

impl TaskCoordinationService {
    async fn work_unit_repository_scope_snapshot(
        &self,
        work_units: &[db::WorkUnit],
    ) -> Result<BTreeMap<String, Vec<Vec<String>>>> {
        let mut scopes = BTreeMap::<String, Vec<Vec<String>>>::new();
        for work_unit in work_units {
            let Some(db::WorkUnitProvenance::Artifact(artifact_id)) = work_unit.provenance.as_ref()
            else {
                continue;
            };
            let Some(plan_record) =
                TaskCoordinationRepo::get_task_coordination_plan(&*self.db, artifact_id).await?
            else {
                continue;
            };
            let artifact = CollaborationRepo::get_artifact(&*self.db, artifact_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::invalid_operation(format!(
                        "plan-backed WorkUnit {} has no source Artifact",
                        work_unit.id
                    ))
                })?;
            let content = artifact.content.as_deref().ok_or_else(|| {
                ServiceError::invalid_operation(format!(
                    "plan-backed WorkUnit {} has no inline source plan",
                    work_unit.id
                ))
            })?;
            let plan = self.parse_plan(content)?;
            if plan.request_id != plan_record.request_id || plan.task_id != work_unit.task_id {
                return Err(ServiceError::invalid_operation(format!(
                    "plan-backed WorkUnit {} has inconsistent source provenance",
                    work_unit.id
                )));
            }
            let mut found_key = false;
            for planned_unit in &plan.work_units {
                if super::deterministic_work_unit_id(&plan.request_id, &planned_unit.key)?
                    == work_unit.id
                {
                    let selected_repository_ids =
                        WorkUnitRepo::list_repository_ids(&*self.db, &work_unit.id).await?;
                    let repository_set = match db::TaskRepositorySetRepo::get_for_work_unit(
                        &*self.db,
                        &work_unit.id,
                        &work_unit.task_id,
                    )
                    .await?
                    {
                        Some(repository_set) => Some(repository_set),
                        None => {
                            db::TaskRepositorySetRepo::get_current(&*self.db, &work_unit.task_id)
                                .await?
                        }
                    };
                    let effective_repository_ids = if selected_repository_ids.is_empty() {
                        repository_set
                            .into_iter()
                            .flat_map(|repository_set| repository_set.repositories)
                            .filter(|repository| {
                                matches!(
                                    repository.role,
                                    db::TaskRepositoryRole::Primary
                                        | db::TaskRepositoryRole::Affected
                                )
                            })
                            .map(|repository| repository.repo_id)
                            .collect()
                    } else {
                        selected_repository_ids
                    };
                    scopes
                        .entry(planned_unit.key.clone())
                        .or_default()
                        .push(effective_repository_ids);
                    found_key = true;
                    break;
                }
            }
            if !found_key {
                return Err(ServiceError::invalid_operation(format!(
                    "plan-backed WorkUnit {} has no matching key in its source plan",
                    work_unit.id
                )));
            }
        }
        Ok(scopes)
    }

    async fn repository_set_snapshot(
        &self,
        task_id: &str,
    ) -> Result<(i64, Option<String>, Vec<ReplanRepositoryMember>)> {
        let revision = db::TaskRepositorySetRepo::get_current(&*self.db, task_id)
            .await?
            .ok_or_else(|| {
                ServiceError::not_found("Task repository-set revision", task_id.to_owned())
            })?;
        let members = revision
            .repositories
            .iter()
            .map(|repository| ReplanRepositoryMember {
                repo_id: repository.repo_id.clone(),
                role: match repository.role {
                    db::TaskRepositoryRole::Primary => api_types::TaskRepositoryRole::Primary,
                    db::TaskRepositoryRole::Affected => api_types::TaskRepositoryRole::Affected,
                    db::TaskRepositoryRole::Context => api_types::TaskRepositoryRole::Context,
                },
                position: repository.position,
            })
            .collect();
        Ok((revision.revision, revision.digest, members))
    }

    pub(super) async fn apply_propose_replan(
        &self,
        source: &TaskCoordinationRequest,
        execution: &db::Execution,
        output: ProposeReplanOutput,
    ) -> Result<()> {
        output.validate().map_err(ServiceError::from)?;
        self.validate_turn_authority(source).await?;
        if source.execution_id.as_deref() != Some(execution.id.as_str())
            || execution.task_id != source.task_id
            || !matches!(execution.actor_ref(), Some(db::ActorRef::Agent(_)))
        {
            return Err(crate::task_coordination::output_not_permitted(
                "propose_replan must come from this exact coordinator Agent Execution",
            ));
        }
        let coordination = self.require_coordination(&source.task_id).await?;
        let authority =
            db::ExecutionTaskRoleAuthorityRepo::get_by_execution_id(&*self.db, &execution.id)
                .await?;
        let exact_coordinator_execution = coordination
            .coordinator_actor
            .as_ref()
            .is_some_and(|actor| execution.actor_ref().as_ref() == Some(actor))
            && coordination
                .coordinator_task_role_id
                .as_deref()
                .zip(authority.as_ref())
                .is_some_and(|(role_id, authority)| {
                    authority.task_role_id == role_id
                        && authority.actor_kind == db::ActorKind::Agent
                        && coordination
                            .coordinator_actor
                            .as_ref()
                            .is_some_and(|actor| authority.actor_id == actor.id())
                });
        if !exact_coordinator_execution {
            return Err(crate::task_coordination::output_not_permitted(
                "propose_replan must come from the exact authorized coordinator TaskRole and Actor",
            ));
        }
        let envelope = self.envelope(&coordination)?;
        if !envelope.allow_coordinator_replan {
            return Err(crate::task_coordination::output_not_permitted(
                "propose_replan requires envelope.allow_coordinator_replan",
            ));
        }
        let service = HumanDecisionService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        for link in &output.evidence_links {
            service
                .validate_evidence_link(&source.task_id, link)
                .await?;
        }

        if let Some(existing) =
            TaskCoordinatorReplanRepo::list_coordinator_replans_by_task(&*self.db, &source.task_id)
                .await?
                .into_iter()
                .find(|row| {
                    row.source_request_id == source.id && row.producer_execution_id == execution.id
                })
        {
            self.ensure_replan_plan_request(&existing, &output, None)
                .await?;
            return self.ensure_replan_proposed_event(&existing).await;
        }

        let task = TaskRepo::get_by_id(&*self.db, &source.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", source.task_id.clone()))?;
        let team = self.current_team(&source.task_id).await?;
        let work_units = WorkUnitRepo::list_by_task(&*self.db, &source.task_id).await?;
        let original_work_unit_repository_ids_by_key = self
            .work_unit_repository_scope_snapshot(&work_units)
            .await?;
        let (repository_set_revision, repository_set_digest, original_repository_set) =
            self.repository_set_snapshot(&task.id).await?;
        let original = ReplanOriginalScope {
            objective_digest: super::external_feedback::task_objective_digest(
                &task.title,
                task.description.as_deref(),
            ),
            current_objective_digest: String::new(),
            original_envelope: envelope.clone(),
            team_roles: team
                .iter()
                .map(|role| {
                    db::canonical_task_role_name(&role.role)
                        .map(|name| name.to_string())
                        .unwrap_or_else(|| role.role.clone())
                })
                .collect(),
            team_actors: team
                .iter()
                .flat_map(|role| {
                    role.members
                        .iter()
                        .map(|member| replan_actor_key(&member.actor_ref))
                })
                .collect(),
            existing_work_units: work_units
                .iter()
                .filter(|unit| unit.status != db::WorkUnitStatus::Cancelled)
                .count(),
            original_work_unit_scopes: work_units
                .iter()
                .filter(|unit| unit.status != db::WorkUnitStatus::Cancelled)
                .map(|unit| unit.scope.clone())
                .collect(),
            original_repository_set,
            original_work_unit_repository_ids_by_key,
            repository_set_revision,
            repository_set_digest: repository_set_digest.clone(),
            current_repository_set_revision: repository_set_revision,
            current_repository_set_digest: repository_set_digest,
            repo_id: task.repo_id.clone(),
            current_repo_id: task.repo_id.clone(),
        };
        let original_json = serde_json::to_value(&original)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let replan_id = new_uuid_v4();
        let now = now_rfc3339();
        let evidence_links_json = serde_json::to_string(&output.evidence_links)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let row = TaskCoordinatorReplanRepo::create_coordinator_replan(
            &*self.db,
            CreateCoordinatorReplan {
                id: replan_id.clone(),
                task_id: source.task_id.clone(),
                authorization_version: source.authorization_version,
                source_request_id: source.id.clone(),
                producer_execution_id: execution.id.clone(),
                reason: output.reason.clone(),
                evidence_links_json,
                created_at: now,
            },
        )
        .await
        .map_err(map_replan_db_error)?;

        self.ensure_replan_plan_request(&row, &output, Some(original_json))
            .await?;
        self.ensure_replan_proposed_event(&row).await
    }

    async fn ensure_replan_proposed_event(&self, row: &db::TaskCoordinatorReplan) -> Result<()> {
        let dedupe_key = format!("task.coordinator_replan_proposed:{}", row.id);
        if DomainEventRepo::get_event_by_dedupe(&*self.db, &dedupe_key)
            .await?
            .is_some()
        {
            return Ok(());
        }
        let current = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, &row.id)
            .await?
            .filter(|current| current.task_id == row.task_id)
            .ok_or_else(|| ServiceError::not_found("coordinator replan", row.id.clone()))?;
        let coordination = self.require_coordination(&row.task_id).await?;
        let execution = db::ExecutionRepo::get_by_id(&*self.db, &row.producer_execution_id)
            .await?
            .filter(|execution| {
                execution.task_id == row.task_id
                    && matches!(execution.actor_ref(), Some(db::ActorRef::Agent(_)))
            })
            .ok_or_else(|| invalid_plan("coordinator replan has no exact Agent producer"))?;
        let event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "task.coordinator_replan_proposed".to_owned(),
            entity_type: "task_coordinator_replan".to_owned(),
            entity_id: current.id.clone(),
            actor_type: "agent".to_owned(),
            actor_id: execution.actor_id,
            scope_type: "task".to_owned(),
            scope_id: current.task_id.clone(),
            correlation_id: current.id.clone(),
            causation_id: Some(current.producer_execution_id.clone()),
            causation_depth: 1,
            dedupe_key: Some(dedupe_key),
            payload_json: json!({
                "task_id": current.task_id,
                "coordinator_replan_id": current.id,
                "ordinal": current.ordinal,
                "plan_request_id": current.plan_request_id,
                "authorization_version": current.authorization_version,
                "envelope_digest": coordination.envelope_digest,
                "state": current.state.to_string(),
            })
            .to_string(),
            created_at: now_rfc3339(),
        };
        let committed = DomainEventRepo::append_event(&*self.db, event).await?;
        DomainEventService::publish_committed_hint(&self.event_bus, &committed);
        Ok(())
    }

    async fn ensure_replan_plan_request(
        &self,
        row: &db::TaskCoordinatorReplan,
        output: &ProposeReplanOutput,
        original_snapshot: Option<Value>,
    ) -> Result<()> {
        let key = format!("coordinator-replan:{}", row.id);
        let existing_request = TaskCoordinationRepo::get_task_coordination_request_by_key(
            &*self.db,
            &row.task_id,
            &key,
        )
        .await?;
        if existing_request.is_none() {
            let coordination = self.require_coordination(&row.task_id).await?;
            let task = TaskRepo::get_by_id(&*self.db, &row.task_id, false)
                .await?
                .ok_or_else(|| ServiceError::not_found("task", row.task_id.clone()))?;
            let brief = CoordinationBrief {
                objective: format!("Replan Task {}: {}", task.id, row.reason),
                constraints: Some(format!(
                    "Propose a new WorkUnit plan for the exact Task objective. Request: {}",
                    output.plan_request.instructions
                )),
                acceptance_criteria: vec![
                    "Return a valid WorkUnitPlan for this exact request and authorization"
                        .to_owned(),
                    "Do not widen Task objective, team, paths, or envelope flags. Include repository_set only to explicitly propose a complete new repository set; it must preserve the primary and any change requires the exact Human decision"
                        .to_owned(),
                ],
                context: Some(
                    json!({
                        "coordinator_replan_id": row.id,
                        "source_request_id": row.source_request_id,
                        "producer_execution_id": row.producer_execution_id,
                        "replan_original_scope": match original_snapshot {
                            Some(snapshot) => snapshot,
                            None => self.original_scope_for_replan(row).await?,
                        },
                        "reason": row.reason,
                        "evidence_links": serde_json::from_str::<Value>(&row.evidence_links_json)
                            .unwrap_or(Value::Array(Vec::new())),
                    })
                    .to_string(),
                ),
            };
            let requested_by = coordination.requested_by.as_deref().ok_or_else(|| {
                coordination_error(
                    "coordination.not_authorized",
                    "coordination has no authorizer",
                )
            })?;
            let has_batch =
                TaskCoordinationRepo::has_current_project_plan_batch_coordination_authorization(
                    &*self.db,
                    &row.task_id,
                )
                .await?;
            let binding = if has_batch {
                Some(
                    self.project_batch_binding_for_task_origin(
                        &row.task_id,
                        requested_by,
                        task.version,
                        coordination.authorization_version,
                        db::ProjectPlanBatchRequestOrigin::CoordinatorReplan {
                            producer_execution_id: row.producer_execution_id.clone(),
                        },
                    )
                    .await?
                    .ok_or(db::DbError::ProjectPlanBatchInactive)?,
                )
            } else {
                None
            };
            let input = TurnRequestInput {
                expected_task_version: task.version,
                expected_coordination_version: coordination.authorization_version,
                brief,
                idempotency_key: key.clone(),
            };
            match binding {
                Some(binding) => {
                    self.request_project_batch_turn(&row.task_id, requested_by, input, binding)
                        .await?;
                }
                None => {
                    self.request_turn(&row.task_id, requested_by, input).await?;
                }
            }
        }
        let request = TaskCoordinationRepo::get_task_coordination_request_by_key(
            &*self.db,
            &row.task_id,
            &key,
        )
        .await?
        .ok_or_else(|| invalid_plan("coordinator replan request was not persisted"))?;
        let latest = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, &row.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("coordinator replan", row.id.clone()))?;
        if latest.state == CoordinatorReplanState::Proposed {
            TaskCoordinatorReplanRepo::transition_coordinator_replan(
                &*self.db,
                TransitionCoordinatorReplan {
                    id: row.id.clone(),
                    expected_version: latest.version,
                    to_state: CoordinatorReplanState::Planning,
                    plan_request_id: Some(request.id),
                    classification: None,
                    classification_facts_json: None,
                    human_decision_id: None,
                    updated_at: now_rfc3339(),
                },
            )
            .await
            .map_err(map_replan_db_error)?;
        }
        Ok(())
    }

    async fn original_scope_for_replan(&self, row: &db::TaskCoordinatorReplan) -> Result<Value> {
        let request_id = row.plan_request_id.as_deref().unwrap_or_default();
        if !request_id.is_empty() {
            if let Some(request) =
                TaskCoordinationRepo::get_task_coordination_request(&*self.db, request_id).await?
            {
                let brief = self.parse_brief(&request)?;
                if let Some((_, scope)) = parse_replan_context(&brief)? {
                    return serde_json::to_value(scope)
                        .map_err(|error| ServiceError::invalid_operation(error.to_string()));
                }
            }
        }
        let source =
            TaskCoordinationRepo::get_task_coordination_request(&*self.db, &row.source_request_id)
                .await?
                .ok_or_else(|| {
                    ServiceError::not_found("coordination request", row.source_request_id.clone())
                })?;
        let coordination = self.require_coordination(&row.task_id).await?;
        let envelope = self.envelope(&coordination)?;
        if source.authorization_version != row.authorization_version
            || coordination.authorization_version != row.authorization_version
            || coordination.mode != source.mode
            || coordination.mode == TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
        {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "coordinator replan source no longer matches its authorization snapshot",
            ));
        }
        let task = TaskRepo::get_by_id(&*self.db, &row.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", row.task_id.clone()))?;
        let team = self.current_team(&row.task_id).await?;
        let work_units = WorkUnitRepo::list_by_task(&*self.db, &row.task_id).await?;
        let original_work_unit_repository_ids_by_key = self
            .work_unit_repository_scope_snapshot(&work_units)
            .await?;
        let (repository_set_revision, repository_set_digest, original_repository_set) =
            self.repository_set_snapshot(&task.id).await?;
        let scope = ReplanOriginalScope {
            objective_digest: super::external_feedback::task_objective_digest(
                &task.title,
                task.description.as_deref(),
            ),
            current_objective_digest: String::new(),
            original_envelope: envelope,
            team_roles: team
                .iter()
                .map(|role| {
                    db::canonical_task_role_name(&role.role)
                        .map(|name| name.to_string())
                        .unwrap_or_else(|| role.role.clone())
                })
                .collect(),
            team_actors: team
                .iter()
                .flat_map(|role| {
                    role.members
                        .iter()
                        .map(|member| replan_actor_key(&member.actor_ref))
                })
                .collect(),
            existing_work_units: work_units
                .iter()
                .filter(|unit| unit.status != db::WorkUnitStatus::Cancelled)
                .count(),
            original_work_unit_scopes: work_units
                .iter()
                .filter(|unit| unit.status != db::WorkUnitStatus::Cancelled)
                .map(|unit| unit.scope.clone())
                .collect(),
            original_repository_set,
            original_work_unit_repository_ids_by_key,
            repository_set_revision,
            repository_set_digest: repository_set_digest.clone(),
            current_repository_set_revision: repository_set_revision,
            current_repository_set_digest: repository_set_digest,
            repo_id: task.repo_id.clone(),
            current_repo_id: task.repo_id,
        };
        serde_json::to_value(scope)
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))
    }

    pub(super) async fn replan_classification(
        &self,
        request: &TaskCoordinationRequest,
        plan: &WorkUnitPlan,
        envelope: &CoordinationEnvelope,
        task: &Task,
    ) -> Result<Option<(String, ReplanClassificationResult)>> {
        let brief = self.parse_brief(request)?;
        let Some((id, mut original)) = parse_replan_context(&brief)? else {
            return Ok(None);
        };
        self.validate_replan_repository_set(request, task, plan)
            .await?;
        original.current_objective_digest = super::external_feedback::task_objective_digest(
            &task.title,
            task.description.as_deref(),
        );
        original.current_repo_id = task.repo_id.clone();
        let (revision, digest, _) = self.repository_set_snapshot(&task.id).await?;
        if plan.repository_set.is_some()
            && (original.repository_set_revision == 0
                || original.repository_set_revision != revision
                || original.repository_set_digest != digest)
        {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "the repository-set base changed or was not captured for this replan",
            ));
        }
        original.current_repository_set_revision = revision;
        original.current_repository_set_digest = digest;
        Ok(Some((id, classify_replan(plan, envelope, &original))))
    }

    pub(super) async fn validate_replan_materialization_decision(
        &self,
        request: &TaskCoordinationRequest,
        artifact_id: &str,
        plan_digest: &str,
        user_id: &str,
    ) -> Result<Option<db::CoordinatorReplanDecisionProof>> {
        let Some((replan_id, original)) = self.replan_context_for_request(request)? else {
            return Ok(None);
        };
        let (current_revision, current_digest, _) =
            self.repository_set_snapshot(&request.task_id).await?;
        if original.repository_set_revision > 0
            && (original.repository_set_revision != current_revision
                || original.repository_set_digest != current_digest)
        {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "the repository-set revision changed after this replan was proposed",
            ));
        }
        let replan = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, &replan_id)
            .await?
            .filter(|row| {
                row.task_id == request.task_id
                    && row.plan_request_id.as_deref() == Some(request.id.as_str())
            })
            .ok_or_else(|| ServiceError::not_found("coordinator replan", replan_id.clone()))?;
        if replan.classification == Some(CoordinatorReplanClassification::ScopeOrAuthorityChange) {
            if replan.state != CoordinatorReplanState::Approved {
                return Err(invalid_plan(
                    "repository or authority changes require an approved exact Human decision",
                ));
            }
            let decision_id = replan
                .human_decision_id
                .as_deref()
                .ok_or_else(|| invalid_plan("approved replan has no exact Human decision"))?;
            let decision = HumanDecisionRepo::get_human_decision_request(&*self.db, decision_id)
                .await?
                .ok_or_else(|| invalid_plan("the exact Human replan decision is not approved"))?;
            validate_exact_replan_decision(
                &decision,
                ExactReplanDecisionBinding {
                    task_id: &request.task_id,
                    replan_id: &replan_id,
                    authorization_version: request.authorization_version,
                    request_id: &request.id,
                    artifact_id,
                    plan_digest,
                    user_id,
                },
            )?;
            return Ok(Some(db::CoordinatorReplanDecisionProof {
                replan_id,
                decision_id: decision.id,
                request_id: request.id.clone(),
                plan_artifact_id: artifact_id.to_owned(),
                plan_digest: plan_digest.to_owned(),
                authorization_version: request.authorization_version,
                approved_by_user_id: user_id.to_owned(),
                base_repository_set_revision: original.repository_set_revision,
                base_repository_set_digest: original.repository_set_digest.clone(),
                base_repo_id: original.repo_id.clone(),
            }));
        }
        Ok(None)
    }

    pub(super) async fn validate_replan_repository_set(
        &self,
        request: &TaskCoordinationRequest,
        task: &Task,
        plan: &WorkUnitPlan,
    ) -> Result<Option<Vec<db::TaskRepositoryWrite>>> {
        let Some(repositories) = plan.repository_set.as_ref() else {
            return Ok(None);
        };
        let Some((_, original)) = self.replan_context_for_request(request)? else {
            return Err(invalid_plan(
                "repository_set is only valid on a coordinator replan plan",
            ));
        };
        if original.repository_set_revision == 0 {
            return Err(coordination_error(
                "coordination.authorization_stale",
                "the repository-set base was not captured for this replan",
            ));
        }
        let primary = repositories
            .iter()
            .filter(|repository| repository.role == api_types::TaskRepositoryRole::Primary)
            .collect::<Vec<_>>();
        let mut ids = std::collections::HashSet::with_capacity(repositories.len());
        if (!repositories.is_empty() && primary.len() != 1)
            || repositories.iter().any(|repository| {
                repository.repo_id.trim().is_empty() || !ids.insert(repository.repo_id.as_str())
            })
        {
            return Err(ServiceError::TaskRepositoriesInvalid);
        }
        if primary
            .first()
            .is_some_and(|repository| task.repo_id.as_deref() != Some(repository.repo_id.as_str()))
        {
            return Err(coordination_error(
                "task.repositories.replan_primary_immutable",
                "coordinator replan cannot change the Task primary repository",
            ));
        }
        if repositories.is_empty() && task.repo_id.is_some() {
            return Err(coordination_error(
                "task.repositories.replan_primary_immutable",
                "coordinator replan cannot remove the Task primary repository",
            ));
        }
        for repository in repositories {
            let belongs_to_project = db::RepoRepo::get_by_id(&*self.db, &repository.repo_id)
                .await?
                .is_some_and(|repo| repo.project_id == task.project_id);
            if !belongs_to_project {
                return Err(ServiceError::TaskRepositoryNotInProject);
            }
        }
        Ok(Some(
            repositories
                .iter()
                .map(|repository| db::TaskRepositoryWrite {
                    repo_id: repository.repo_id.clone(),
                    role: match repository.role {
                        api_types::TaskRepositoryRole::Primary => db::TaskRepositoryRole::Primary,
                        api_types::TaskRepositoryRole::Affected => db::TaskRepositoryRole::Affected,
                        api_types::TaskRepositoryRole::Context => db::TaskRepositoryRole::Context,
                    },
                })
                .collect(),
        ))
    }

    pub(super) fn replan_context_for_request(
        &self,
        request: &TaskCoordinationRequest,
    ) -> Result<Option<(String, ReplanOriginalScope)>> {
        parse_replan_context(&self.parse_brief(request)?)
    }

    pub(super) async fn record_replan_classification(
        &self,
        id: &str,
        next_state: CoordinatorReplanState,
        classification: ReplanClassification,
        facts_json: String,
    ) -> Result<()> {
        let row = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, id)
            .await?
            .ok_or_else(|| ServiceError::not_found("coordinator replan", id.to_owned()))?;
        TaskCoordinatorReplanRepo::transition_coordinator_replan(
            &*self.db,
            TransitionCoordinatorReplan {
                id: id.to_owned(),
                expected_version: row.version,
                to_state: next_state,
                plan_request_id: None,
                classification: Some(match classification {
                    ReplanClassification::WithinEnvelope => {
                        CoordinatorReplanClassification::WithinEnvelope
                    }
                    ReplanClassification::ScopeOrAuthorityChange => {
                        CoordinatorReplanClassification::ScopeOrAuthorityChange
                    }
                }),
                classification_facts_json: Some(facts_json),
                human_decision_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(map_replan_db_error)?;
        Ok(())
    }

    pub(super) async fn mark_coordinator_replan_state(
        &self,
        id: &str,
        state: CoordinatorReplanState,
    ) -> Result<()> {
        let row = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, id)
            .await?
            .ok_or_else(|| ServiceError::not_found("coordinator replan", id.to_owned()))?;
        if row.state == state {
            return Ok(());
        }
        TaskCoordinatorReplanRepo::transition_coordinator_replan(
            &*self.db,
            TransitionCoordinatorReplan {
                id: id.to_owned(),
                expected_version: row.version,
                to_state: state,
                plan_request_id: None,
                classification: None,
                classification_facts_json: None,
                human_decision_id: None,
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(map_replan_db_error)?;
        Ok(())
    }

    pub(super) async fn reconcile_replan_plan_completion(
        &self,
        request: &TaskCoordinationRequest,
        execution: &db::Execution,
        plan_row: &db::TaskCoordinationPlan,
    ) -> Result<bool> {
        let Some((replan_id, _)) = self.replan_context_for_request(request)? else {
            return Ok(false);
        };
        let Some(replan) =
            TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, &replan_id).await?
        else {
            return Err(ServiceError::not_found("coordinator replan", replan_id));
        };
        if replan.state != CoordinatorReplanState::Planning {
            return Ok(replan.state == CoordinatorReplanState::AwaitingHuman
                && replan.human_decision_id.is_some());
        }
        let artifact = CollaborationRepo::get_artifact(&*self.db, &plan_row.artifact_id)
            .await?
            .filter(|artifact| artifact.task_id == request.task_id)
            .ok_or_else(|| ServiceError::not_found("artifact", plan_row.artifact_id.clone()))?;
        let content = artifact
            .content
            .as_deref()
            .ok_or_else(|| invalid_plan("replan plan Artifact has no inline content"))?;
        let parsed = self.parse_plan(content)?;
        let coordination = self.require_coordination(&request.task_id).await?;
        let envelope = self.envelope(&coordination)?;
        let task = TaskRepo::get_by_id(&*self.db, &request.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", request.task_id.clone()))?;
        let Some((_, classification)) = self
            .replan_classification(request, &parsed, &envelope, &task)
            .await?
        else {
            return Err(invalid_plan("replan plan request lost its exact origin"));
        };
        if classification.classification == ReplanClassification::ScopeOrAuthorityChange {
            if plan_row.state == TaskCoordinationPlanState::Approved {
                return Err(invalid_plan("out-of-envelope replan was already approved"));
            }
            self.raise_replan_scope_decision(RaiseReplanScopeDecisionInput {
                request,
                execution,
                artifact_id: &plan_row.artifact_id,
                artifact_digest: &plan_row.plan_digest,
                replan_id: &replan_id,
                classification: classification.classification,
                facts_json: classification.classification_facts_json(),
            })
            .await?;
            return Ok(true);
        }
        let next_state = if plan_row.state == TaskCoordinationPlanState::Approved {
            CoordinatorReplanState::Approved
        } else {
            CoordinatorReplanState::AwaitingHuman
        };
        self.record_replan_classification(
            &replan_id,
            next_state,
            classification.classification,
            classification.classification_facts_json(),
        )
        .await?;
        Ok(false)
    }

    pub(super) async fn raise_replan_scope_decision(
        &self,
        input: RaiseReplanScopeDecisionInput<'_>,
    ) -> Result<()> {
        let RaiseReplanScopeDecisionInput {
            request,
            execution,
            artifact_id,
            artifact_digest,
            replan_id,
            classification,
            facts_json,
        } = input;
        let row = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, replan_id)
            .await?
            .filter(|row| row.task_id == request.task_id)
            .ok_or_else(|| ServiceError::not_found("coordinator replan", replan_id.to_owned()))?;
        let raised = HumanDecisionService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .raise(RaiseHumanDecisionInput {
                task_id: request.task_id.clone(),
                source_kind: HumanDecisionSourceKind::CoordinatorReplan,
                source_id: replan_id.to_owned(),
                raised_by_execution_id: Some(execution.id.clone()),
                authorization_version: Some(request.authorization_version),
                ask: crate::coordinator_outputs::AskHumanOutput {
                    question: format!("Approve the scope change in replan {}?", row.ordinal),
                    options: vec![
                        crate::coordinator_outputs::HumanDecisionOption {
                            id: "approve_replan".to_owned(),
                            label: "Approve replan".to_owned(),
                            description: "Approve this exact plan and its measured scope changes"
                                .to_owned(),
                            consequence:
                                "The exact plan may be materialized under current authorization"
                                    .to_owned(),
                            authority_effect: db::AuthorityEffect::Scope,
                        },
                        crate::coordinator_outputs::HumanDecisionOption {
                            id: "reject".to_owned(),
                            label: "Reject".to_owned(),
                            description: "Reject this exact replan".to_owned(),
                            consequence: "The plan will not be materialized".to_owned(),
                            authority_effect: db::AuthorityEffect::None,
                        },
                        crate::coordinator_outputs::HumanDecisionOption {
                            id: "revise_scope".to_owned(),
                            label: "Revise scope".to_owned(),
                            description:
                                "Return the Task to its owner for a new scope and authorization"
                                    .to_owned(),
                            consequence: "This plan will not be materialized".to_owned(),
                            authority_effect: db::AuthorityEffect::Scope,
                        },
                    ],
                    recommended_option_id: None,
                    recommendation_rationale: None,
                    evidence_links: vec![EvidenceLink {
                        kind: EvidenceKind::Artifact,
                        id: artifact_id.to_owned(),
                        digest: Some(artifact_digest.to_owned()),
                    }],
                },
                resume_binding: ResumeBinding {
                    request_id: request.id.clone(),
                    request_kind: TaskCoordinationRequestKind::WorkUnitPlan,
                    authorization_version: request.authorization_version,
                    source_subject: json!({
                        "coordinator_replan_id": replan_id,
                        "plan_request_id": request.id,
                        "plan_artifact_id": artifact_id,
                        "plan_digest": artifact_digest,
                    }),
                },
            })
            .await?;
        TaskCoordinatorReplanRepo::transition_coordinator_replan(
            &*self.db,
            TransitionCoordinatorReplan {
                id: replan_id.to_owned(),
                expected_version: row.version,
                to_state: CoordinatorReplanState::AwaitingHuman,
                plan_request_id: None,
                classification: Some(match classification {
                    ReplanClassification::WithinEnvelope => {
                        CoordinatorReplanClassification::WithinEnvelope
                    }
                    ReplanClassification::ScopeOrAuthorityChange => {
                        CoordinatorReplanClassification::ScopeOrAuthorityChange
                    }
                }),
                classification_facts_json: Some(facts_json),
                human_decision_id: Some(raised.decision.id),
                updated_at: now_rfc3339(),
            },
        )
        .await
        .map_err(map_replan_db_error)?;
        Ok(())
    }

    pub(super) async fn complete_replan_human_resume(
        &self,
        request: &TaskCoordinationRequest,
        decision: &db::HumanDecisionRequest,
        paused: &TaskCoordinationRequest,
        output_decision: &str,
    ) -> Result<()> {
        let replan_id = decision.source_id.as_str();
        let replan = TaskCoordinatorReplanRepo::get_coordinator_replan(&*self.db, replan_id)
            .await?
            .filter(|row| {
                row.task_id == request.task_id
                    && row.human_decision_id.as_deref() == Some(decision.id.as_str())
                    && row.plan_request_id.as_deref() == Some(paused.id.as_str())
            })
            .ok_or_else(|| invalid_plan("answered replan does not match its exact pending plan"))?;
        let answered = decision.answered_option_id.as_deref().unwrap_or_default();
        if output_decision != answered
            || !matches!(answered, "approve_replan" | "reject" | "revise_scope")
        {
            return Err(invalid_plan(
                "replan resume must follow the exact Human option",
            ));
        }
        if matches!(
            replan.state,
            CoordinatorReplanState::Materialized
                | CoordinatorReplanState::Rejected
                | CoordinatorReplanState::Failed
        ) {
            return Ok(());
        }
        if replan.state != CoordinatorReplanState::AwaitingHuman {
            return Err(invalid_plan(
                "coordinator replan is not awaiting its Human decision",
            ));
        }
        let plan =
            TaskCoordinationRepo::list_task_coordination_plans(&*self.db, &request.task_id, 100)
                .await?
                .into_iter()
                .find(|plan| plan.request_id == paused.id)
                .ok_or_else(|| invalid_plan("replan request has no exact WorkUnitPlan"))?;
        if answered == "approve_replan" {
            let user_id = decision
                .answered_by_user_id
                .as_deref()
                .ok_or_else(|| invalid_plan("replan answer has no Human"))?;
            let approved = self
                .approve_plan(
                    &request.task_id,
                    &plan.artifact_id,
                    user_id,
                    PlanDecisionInput {
                        expected_coordination_version: request.authorization_version,
                        expected_plan_digest: plan.plan_digest.clone(),
                        approve: true,
                        rationale: decision.answer_note.clone(),
                        idempotency_key: format!("human-replan:{}", decision.id),
                    },
                )
                .await?;
            let coordination = self.require_coordination(&request.task_id).await?;
            self.materialize(
                &request.task_id,
                &plan.artifact_id,
                user_id,
                MaterializeInput {
                    expected_task_version: approved.parsed.task_version,
                    expected_coordination_version: coordination.authorization_version,
                    expected_plan_digest: plan.plan_digest,
                    expected_plan_version: approved.plan.version,
                    expected_request_version: paused.version,
                    idempotency_key: format!("human-replan-materialize:{}", decision.id),
                },
            )
            .await?;
            self.mark_coordinator_replan_state(replan_id, CoordinatorReplanState::Materialized)
                .await?;
            return Ok(());
        } else if answered == "reject" {
            let user_id = decision
                .answered_by_user_id
                .as_deref()
                .ok_or_else(|| invalid_plan("replan answer has no Human"))?;
            self.approve_plan(
                &request.task_id,
                &plan.artifact_id,
                user_id,
                PlanDecisionInput {
                    expected_coordination_version: request.authorization_version,
                    expected_plan_digest: plan.plan_digest.clone(),
                    approve: false,
                    rationale: decision.answer_note.clone(),
                    idempotency_key: format!("human-replan:{}", decision.id),
                },
            )
            .await?;
            self.mark_coordinator_replan_state(replan_id, CoordinatorReplanState::Rejected)
                .await?;
        } else {
            self.mark_coordinator_replan_state(replan_id, CoordinatorReplanState::Failed)
                .await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        validate_exact_replan_decision, ExactReplanDecisionBinding, TaskCoordinationRequestKind,
    };
    use crate::human_decision_service::ResumeBinding;

    fn expected_binding<'a>(
        replan_id: &'a str,
        authorization_version: i64,
        artifact_id: &'a str,
        user_id: &'a str,
    ) -> ExactReplanDecisionBinding<'a> {
        ExactReplanDecisionBinding {
            task_id: "task-1",
            replan_id,
            authorization_version,
            request_id: "request-1",
            artifact_id,
            plan_digest: "digest-1",
            user_id,
        }
    }

    fn approved_decision() -> db::HumanDecisionRequest {
        db::HumanDecisionRequest {
            id: "decision-1".to_owned(),
            task_id: "task-1".to_owned(),
            source_kind: db::HumanDecisionSourceKind::CoordinatorReplan,
            source_id: "replan-1".to_owned(),
            raised_by_execution_id: Some("execution-1".to_owned()),
            authorization_version: Some(4),
            question: "Approve this exact replan?".to_owned(),
            options_json: "[]".to_owned(),
            recommended_option_id: None,
            recommendation_rationale: None,
            evidence_links_json: "[]".to_owned(),
            max_authority_effect: db::AuthorityEffect::Scope,
            state: db::HumanDecisionState::Answered,
            answered_by_user_id: Some("user-1".to_owned()),
            answered_option_id: Some("approve_replan".to_owned()),
            answer_note: None,
            answered_at: Some("2026-11-21T00:00:00Z".to_owned()),
            resume_request_id: Some("resume-request-1".to_owned()),
            resume_binding_json: serde_json::to_string(&ResumeBinding {
                request_id: "request-1".to_owned(),
                request_kind: TaskCoordinationRequestKind::WorkUnitPlan,
                authorization_version: 4,
                source_subject: serde_json::json!({
                    "coordinator_replan_id": "replan-1",
                    "plan_request_id": "request-1",
                    "plan_artifact_id": "artifact-1",
                    "plan_digest": "digest-1",
                }),
            })
            .expect("resume binding serializes"),
            version: 2,
            created_at: "2026-11-21T00:00:00Z".to_owned(),
            updated_at: "2026-11-21T00:00:00Z".to_owned(),
        }
    }

    #[test]
    fn multirepo_lifecycle_replan_after_execution_requires_exact_human_decision() {
        let decision = approved_decision();
        validate_exact_replan_decision(
            &decision,
            expected_binding("replan-1", 4, "artifact-1", "user-1"),
        )
        .expect("the exact approving Human may materialize the bound plan");

        assert!(validate_exact_replan_decision(
            &decision,
            expected_binding("other-replan", 4, "artifact-1", "user-1"),
        )
        .is_err());
        assert!(validate_exact_replan_decision(
            &decision,
            expected_binding("replan-1", 5, "artifact-1", "user-1"),
        )
        .is_err());
        assert!(validate_exact_replan_decision(
            &decision,
            expected_binding("replan-1", 4, "other-artifact", "user-1"),
        )
        .is_err());
        let mut rejected = decision.clone();
        rejected.answered_option_id = Some("reject".to_owned());
        assert!(validate_exact_replan_decision(
            &rejected,
            expected_binding("replan-1", 4, "artifact-1", "user-1"),
        )
        .is_err());
        assert!(validate_exact_replan_decision(
            &decision,
            expected_binding("replan-1", 4, "artifact-1", "different-user"),
        )
        .is_err());
    }
}
