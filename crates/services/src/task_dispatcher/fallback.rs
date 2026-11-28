use std::sync::Arc;

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use db::{
    AgentRepo, CreateDomainEvent, CreateWorkUnitFallbackReceipt, DomainEventRepo, ExecutionRepo,
    ProjectRepo, TaskCoordinationRepo, TaskLifecycleRepo, TaskRepo, TaskRoleRepo,
    WorkUnitFallbackReceiptRepo, WorkUnitRepo,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::{
    domain_event_service::DomainEventService,
    task_coordination::{CoordinationEnvelope, EnvelopePhaseThreePinContext},
    task_lifecycle::{LifecycleCause, TaskLifecycleService, TransitionLifecycleInput},
    Result, ServiceError,
};

use super::TaskDispatcher;

const ROUTE_EXHAUSTION_OUTCOMES: &[&str] = &["usage_exhausted", "unavailable", "skipped_cooldown"];

#[derive(Debug, Clone)]
struct UnavailableRoute {
    attempts: Vec<Value>,
    retry_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryHintAction {
    Wait,
    RetrySameAgent,
    CrossAgent,
}

#[derive(Debug, Clone)]
struct FallbackContext {
    task_id: String,
    project_id: String,
    work_unit_id: String,
    task_role_id: String,
    origin_execution_id: String,
    origin_agent_id: String,
    work_unit_version: i64,
    coordination: db::TaskCoordination,
    envelope: CoordinationEnvelope,
    route_attempts_digest: String,
}

#[derive(Debug, Clone)]
struct FallbackCandidate {
    chain_index: i64,
    grant: crate::task_coordination::EnvelopeFallbackAgent,
}

impl TaskDispatcher {
    /// Process only exact failed WorkUnit Executions whose whole same-Agent
    /// route is exhausted. A fallback receipt is the durable authority for
    /// each cross-Agent reassignment; retries and event projection are replay
    /// safe across dispatcher ticks.
    pub(super) async fn process_work_unit_fallbacks(&self) -> Result<u64> {
        let execution_ids: Vec<String> = sqlx::query_scalar(
            "SELECT execution.id
             FROM execution
             JOIN task_coordination coordination
               ON coordination.task_id = execution.task_id
             JOIN task_lifecycle lifecycle
               ON lifecycle.task_id = execution.task_id
             JOIN task_dispatch dispatch
               ON dispatch.task_id = execution.task_id
             WHERE execution.status = 'failed'
               AND execution.work_unit_id IS NOT NULL
               AND coordination.mode != 'manual'
               AND coordination.revoked_at IS NULL
               AND coordination.team_stale = 0
               AND lifecycle.state = 'blocked'
               AND lifecycle.reason_kind = 'executor_unavailable'
               AND lifecycle.reason_ref = execution.id
               AND dispatch.state = 'running'
               AND (
                   NOT EXISTS (
                       SELECT 1 FROM work_unit_fallback_receipt receipt
                       WHERE receipt.origin_execution_id = execution.id
                   )
                   OR EXISTS (
                       SELECT 1 FROM work_unit_fallback_receipt receipt
                       WHERE receipt.origin_execution_id = execution.id
                         AND receipt.authorization_version = coordination.authorization_version
                         AND receipt.envelope_digest = coordination.envelope_digest
                   )
               )
               AND CASE
                   WHEN json_valid(execution.executor_config_snapshot_json)
                   THEN json_extract(
                       execution.executor_config_snapshot_json,
                       '$.routing.disposition.failure_class'
                   )
                   ELSE NULL
               END = 'executor_unavailable'
             ORDER BY execution.updated_at, execution.id",
        )
        .fetch_all(self.db.pool())
        .await?;

        let mut processed = 0;
        for execution_id in execution_ids {
            if self.is_stopped() {
                break;
            }
            match self
                .process_unavailable_work_unit_execution(&execution_id)
                .await
            {
                Ok(true) => processed += 1,
                Ok(false) => {}
                Err(ServiceError::Db(db::DbError::VersionConflict))
                | Err(ServiceError::Db(db::DbError::TaskVersionConflict { .. })) => {
                    tracing::debug!(
                        execution_id,
                        "fallback dispatcher lost an optimistic version race"
                    );
                }
                Err(error) => {
                    tracing::warn!(
                        execution_id,
                        %error,
                        "fallback dispatcher could not resolve unavailable Execution"
                    );
                }
            }
        }
        Ok(processed)
    }

    async fn process_unavailable_work_unit_execution(&self, execution_id: &str) -> Result<bool> {
        let Some(execution) = ExecutionRepo::get_by_id(&*self.db, execution_id).await? else {
            return Ok(false);
        };
        if execution.status != db::ExecutionStatus::Failed {
            return Ok(false);
        }
        let Some(route) = unavailable_route(&execution)? else {
            return Ok(false);
        };
        if route.attempts.is_empty() || !route_attempts_exhausted(&route.attempts) {
            return Ok(false);
        }
        if execution_has_uncertain_provider_start(&self.db, &execution.id).await? {
            return Ok(false);
        }

        if let Some(receipt) =
            WorkUnitFallbackReceiptRepo::get_work_unit_fallback_receipt_by_origin_execution(
                &*self.db,
                &execution.id,
            )
            .await?
        {
            return self.complete_fallback_receipt(&execution, &receipt).await;
        }

        if DomainEventRepo::get_event_by_dedupe(
            &*self.db,
            &format!("task.work_unit_fallback:{}", execution.id),
        )
        .await?
        .is_some()
        {
            return Ok(false);
        }
        let Some(context) = self.load_fallback_context(&execution, &route).await? else {
            return Ok(false);
        };
        let candidates = role_fallback_candidates(&context.envelope, &context.task_role_id);
        if candidates.is_empty() {
            self.record_fallback_block(&context, "chain_exhausted", None, 0)
                .await?;
            return Ok(true);
        }
        let Some(work_unit) = WorkUnitRepo::get_by_id(&*self.db, &context.work_unit_id).await?
        else {
            return Ok(false);
        };
        let Some(role) = TaskRoleRepo::get_by_id(&*self.db, &context.task_role_id).await? else {
            return Ok(false);
        };
        if work_unit.task_id != context.task_id
            || work_unit.role != role.role
            || work_unit.version != context.work_unit_version
            || work_unit.assigned_actor
                != Some(db::ActorRef::Agent(context.origin_agent_id.clone()))
            || work_unit.status != db::WorkUnitStatus::Open
        {
            return Ok(false);
        }

        let dispatch_state: Option<String> =
            sqlx::query_scalar("SELECT state FROM task_dispatch WHERE task_id = ?")
                .bind(&context.task_id)
                .fetch_optional(self.db.pool())
                .await?;
        if dispatch_state.as_deref() != Some("running") {
            return Ok(false);
        }
        let running_attempt: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                    SELECT 1 FROM execution
                    WHERE work_unit_id = ? AND status = 'running'
                )",
        )
        .bind(&context.work_unit_id)
        .fetch_one(self.db.pool())
        .await?;
        if running_attempt {
            return Ok(false);
        }

        let last_receipt =
            WorkUnitFallbackReceiptRepo::list_work_unit_fallback_receipts_by_work_unit(
                &*self.db,
                &context.work_unit_id,
            )
            .await?
            .into_iter()
            .filter(|receipt| receipt.task_role_id == context.task_role_id)
            .max_by_key(|receipt| receipt.chain_index);
        if let Some(last_receipt) = last_receipt.as_ref() {
            if last_receipt.to_actor_id != context.origin_agent_id
                || last_receipt.work_unit_version_after != context.work_unit_version
            {
                return Ok(false);
            }
        }
        self.apply_first_or_next_fallback(context, route, work_unit, candidates, last_receipt)
            .await
    }

    async fn load_fallback_context(
        &self,
        execution: &db::Execution,
        route: &UnavailableRoute,
    ) -> Result<Option<FallbackContext>> {
        let (
            Some(work_unit_id),
            Some(work_unit_version),
            Some(db::ActorRef::Agent(origin_agent_id)),
        ) = (
            execution.work_unit_id.as_deref(),
            execution.work_unit_version,
            execution.actor_ref(),
        )
        else {
            return Ok(None);
        };
        if execution.agent_id.as_deref() != Some(origin_agent_id.as_str()) {
            return Ok(None);
        }
        let Some(work_unit) = WorkUnitRepo::get_by_id(&*self.db, work_unit_id).await? else {
            return Ok(None);
        };
        if work_unit.task_id != execution.task_id
            || work_unit.version != work_unit_version
            || work_unit.role != execution.role
            || work_unit.assigned_actor != Some(db::ActorRef::Agent(origin_agent_id.clone()))
            || work_unit.status != db::WorkUnitStatus::Open
        {
            return Ok(None);
        }
        let authority_role_id: Option<String> = sqlx::query_scalar(
            "SELECT task_role_id FROM execution_task_role_authority
             WHERE execution_id = ? AND actor_kind = 'agent' AND actor_id = ?",
        )
        .bind(&execution.id)
        .bind(&origin_agent_id)
        .fetch_optional(self.db.pool())
        .await?;
        let Some(task_role_id) = authority_role_id else {
            return Ok(None);
        };
        let Some(role) = TaskRoleRepo::get_by_id(&*self.db, &task_role_id).await? else {
            return Ok(None);
        };
        if role.task_id != execution.task_id || role.role != work_unit.role {
            return Ok(None);
        }
        let Some(task) = TaskRepo::get_by_id(&*self.db, &execution.task_id, false).await? else {
            return Ok(None);
        };
        if task.failed_json.is_some() || !blocking_metadata_matches(&task, &execution.id) {
            return Ok(None);
        }
        let Some(project) = ProjectRepo::get_by_id(&*self.db, &task.project_id).await? else {
            return Ok(None);
        };
        let Some(coordination) =
            TaskCoordinationRepo::get_task_coordination(&*self.db, &task.id).await?
        else {
            return Ok(None);
        };
        if coordination.mode == db::TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
        {
            return Ok(None);
        }
        let Some(lifecycle) = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &task.id).await?
        else {
            return Ok(None);
        };
        if lifecycle.state != db::TaskLifecycleState::Blocked
            || lifecycle.reason_kind.as_deref() != Some("executor_unavailable")
            || lifecycle.reason_ref.as_deref() != Some(execution.id.as_str())
        {
            return Ok(None);
        }
        let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
            .map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "invalid authorized fallback envelope: {error}"
                ))
            })?;
        envelope.validate()?;
        let canonical_envelope = serde_json::to_string(&envelope).map_err(|error| {
            ServiceError::invalid_operation(format!(
                "invalid authorized fallback envelope: {error}"
            ))
        })?;
        if sha256_hex(canonical_envelope.as_bytes()) != coordination.envelope_digest {
            return Ok(None);
        }
        envelope
            .validate_phase_three_pins(&EnvelopePhaseThreePinContext {
                db: &self.db,
                project_id: &project.id,
                task_id: Some(&task.id),
                coordinator_actor: coordination.coordinator_actor.as_ref(),
            })
            .await?;
        Ok(Some(FallbackContext {
            task_id: task.id,
            project_id: project.id,
            work_unit_id: work_unit.id,
            task_role_id,
            origin_execution_id: execution.id.clone(),
            origin_agent_id,
            work_unit_version,
            coordination,
            envelope,
            route_attempts_digest: route_attempts_digest(&route.attempts)?,
        }))
    }

    async fn apply_first_or_next_fallback(
        &self,
        context: FallbackContext,
        route: UnavailableRoute,
        work_unit: db::WorkUnit,
        candidates: Vec<FallbackCandidate>,
        last_receipt: Option<db::WorkUnitFallbackReceipt>,
    ) -> Result<bool> {
        let next_index = last_receipt
            .as_ref()
            .map(|receipt| receipt.chain_index + 1)
            .unwrap_or(0);
        let candidate = candidates
            .iter()
            .find(|candidate| {
                candidate.chain_index >= next_index
                    && candidate.grant.agent_id != context.origin_agent_id
            })
            .cloned();
        let retry_budget_seconds = candidate
            .as_ref()
            .map(|candidate| candidate.grant.max_wait_seconds)
            .or_else(|| {
                last_receipt.as_ref().and_then(|receipt| {
                    candidates
                        .iter()
                        .find(|candidate| candidate.chain_index == receipt.chain_index)
                        .map(|candidate| candidate.grant.max_wait_seconds)
                })
            })
            .or_else(|| {
                candidates
                    .first()
                    .map(|candidate| candidate.grant.max_wait_seconds)
            });
        if let Some(max_wait_seconds) = retry_budget_seconds {
            match retry_hint_action(route.retry_at.as_deref(), max_wait_seconds, Utc::now())? {
                RetryHintAction::Wait => return Ok(false),
                RetryHintAction::RetrySameAgent => {
                    return self.retry_same_agent(&context, &work_unit).await;
                }
                RetryHintAction::CrossAgent => {}
            }
        }
        let Some(candidate) = candidate else {
            self.record_fallback_block(
                &context,
                "chain_exhausted",
                None,
                i64::try_from(
                    role_fallback_candidates(&context.envelope, &context.task_role_id).len(),
                )
                .unwrap_or(i64::MAX),
            )
            .await?;
            self.block_if_needed(&context, &context.origin_execution_id, None)
                .await?;
            return Ok(true);
        };

        if !self
            .fallback_candidate_is_eligible(&context, &candidate)
            .await?
        {
            self.record_fallback_block(
                &context,
                "fallback.not_authorized",
                Some(candidate.grant.agent_id.as_str()),
                candidate.chain_index,
            )
            .await?;
            self.block_if_needed(&context, &context.origin_execution_id, None)
                .await?;
            return Ok(true);
        }

        let next_version = work_unit.version.checked_add(1).ok_or_else(|| {
            ServiceError::invalid_operation("WorkUnit version cannot be incremented")
        })?;
        let now = db::now_rfc3339();
        let allocation_event = CreateDomainEvent {
            id: db::new_uuid_v4(),
            event_type: "work_unit.allocation_changed".to_owned(),
            entity_type: "work_unit".to_owned(),
            entity_id: work_unit.id.clone(),
            actor_type: "system".to_owned(),
            actor_id: None,
            scope_type: "task".to_owned(),
            scope_id: context.task_id.clone(),
            correlation_id: format!("work-unit-fallback:{}", context.origin_execution_id),
            causation_id: Some(context.origin_execution_id.clone()),
            causation_depth: 1,
            dedupe_key: Some(format!(
                "work-unit-fallback-allocation:{}",
                context.origin_execution_id
            )),
            payload_json: json!({
                "task_id": context.task_id,
                "work_unit_id": work_unit.id,
                "version": next_version,
                "task_role_id": context.task_role_id,
                "from_actor_id": context.origin_agent_id,
                "to_actor_id": candidate.grant.agent_id,
                "origin_execution_id": context.origin_execution_id,
            })
            .to_string(),
            created_at: now.clone(),
        };
        let receipt_input = CreateWorkUnitFallbackReceipt {
            id: db::new_uuid_v4(),
            task_id: context.task_id.clone(),
            work_unit_id: context.work_unit_id.clone(),
            work_unit_version_before: context.work_unit_version,
            work_unit_version_after: next_version,
            origin_execution_id: context.origin_execution_id.clone(),
            failure_class: "executor_unavailable".to_owned(),
            route_attempts_digest: context.route_attempts_digest.clone(),
            task_role_id: context.task_role_id.clone(),
            from_actor_kind: db::ActorKind::Agent,
            from_actor_id: context.origin_agent_id.clone(),
            to_actor_kind: db::ActorKind::Agent,
            to_actor_id: candidate.grant.agent_id.clone(),
            chain_index: candidate.chain_index,
            cost_class: candidate.grant.cost_class,
            authorization_version: context.coordination.authorization_version,
            envelope_digest: context.coordination.envelope_digest.clone(),
            // The durable receipt is inserted before the dispatcher admits a
            // replacement Execution. Its optional execution link remains null.
            new_execution_id: None,
            created_at: now.clone(),
        };
        let (receipt, allocation_event) =
            WorkUnitFallbackReceiptRepo::allocate_work_unit_with_fallback_receipt(
                &*self.db,
                db::AllocateWorkUnit {
                    id: work_unit.id.clone(),
                    expected_version: work_unit.version,
                    role: work_unit.role.clone(),
                    assigned_actor: Some(db::ActorRef::Agent(candidate.grant.agent_id.clone())),
                    updated_at: now,
                },
                allocation_event,
                receipt_input,
            )
            .await?;
        DomainEventService::publish_committed_hint(&self.event_bus, &allocation_event);
        self.record_fallback_receipt_event(&context, &receipt)
            .await?;
        self.activate_fallback(&context, &receipt).await?;
        Ok(true)
    }

    async fn retry_same_agent(
        &self,
        context: &FallbackContext,
        work_unit: &db::WorkUnit,
    ) -> Result<bool> {
        let Some(agent) = AgentRepo::get_by_id(&*self.db, &context.origin_agent_id).await? else {
            return Ok(false);
        };
        if agent.paused {
            return Ok(false);
        }
        let prompt = work_unit
            .execution_instructions
            .clone()
            .filter(|instructions| !instructions.trim().is_empty())
            .unwrap_or_else(|| {
                "Retry the same WorkUnit using its currently assigned Agent profile after the provider retry deadline. Keep the existing TaskRole and WorkUnit scope. Do not switch to a different Agent."
                    .to_owned()
            });
        let execution = self
            .task_service
            .dispatch_role_follow_up_with_agent(
                &context.task_id,
                &work_unit.role,
                context.origin_execution_id.clone(),
                context.origin_agent_id.clone(),
                prompt,
                "executor_unavailable_retry_at",
                format!(
                    "executor-unavailable-same-agent:{}",
                    context.origin_execution_id
                ),
            )
            .await?;
        tracing::info!(
            task_id = %context.task_id,
            work_unit_id = %context.work_unit_id,
            origin_execution_id = %context.origin_execution_id,
            retry_execution_id = %execution.id,
            "same-Agent retry dispatched after retry_at"
        );
        Ok(true)
    }

    async fn complete_fallback_receipt(
        &self,
        execution: &db::Execution,
        receipt: &db::WorkUnitFallbackReceipt,
    ) -> Result<bool> {
        if execution.task_id != receipt.task_id
            || execution.work_unit_id.as_deref() != Some(receipt.work_unit_id.as_str())
        {
            return Ok(false);
        }
        let Some(task) = TaskRepo::get_by_id(&*self.db, &receipt.task_id, false).await? else {
            return Ok(false);
        };
        let Some(work_unit) = WorkUnitRepo::get_by_id(&*self.db, &receipt.work_unit_id).await?
        else {
            return Ok(false);
        };
        if work_unit.version != receipt.work_unit_version_after
            || work_unit.assigned_actor != Some(db::ActorRef::Agent(receipt.to_actor_id.clone()))
        {
            return Ok(false);
        }
        self.ensure_fallback_event(receipt).await?;
        let Some(coordination) =
            TaskCoordinationRepo::get_task_coordination(&*self.db, &receipt.task_id).await?
        else {
            return Ok(false);
        };
        if coordination.mode == db::TaskCoordinationMode::Manual
            || coordination.revoked_at.is_some()
            || coordination.team_stale
            || coordination.authorization_version != receipt.authorization_version
            || coordination.envelope_digest != receipt.envelope_digest
        {
            return Ok(false);
        }
        let envelope: CoordinationEnvelope = serde_json::from_str(&coordination.envelope_json)
            .map_err(|error| {
                ServiceError::invalid_operation(format!("invalid fallback envelope: {error}"))
            })?;
        envelope.validate()?;
        let canonical_envelope = serde_json::to_string(&envelope).map_err(|error| {
            ServiceError::invalid_operation(format!("invalid fallback envelope: {error}"))
        })?;
        if sha256_hex(canonical_envelope.as_bytes()) != coordination.envelope_digest {
            return Ok(false);
        }
        let context = FallbackContext {
            task_id: receipt.task_id.clone(),
            project_id: task.project_id,
            work_unit_id: receipt.work_unit_id.clone(),
            task_role_id: receipt.task_role_id.clone(),
            origin_execution_id: receipt.origin_execution_id.clone(),
            origin_agent_id: receipt.from_actor_id.clone(),
            work_unit_version: receipt.work_unit_version_before,
            coordination,
            envelope,
            route_attempts_digest: receipt.route_attempts_digest.clone(),
        };
        self.activate_fallback(&context, receipt).await?;
        Ok(true)
    }

    async fn activate_fallback(
        &self,
        context: &FallbackContext,
        receipt: &db::WorkUnitFallbackReceipt,
    ) -> Result<()> {
        if context.coordination.mode == db::TaskCoordinationMode::Manual
            || context.coordination.revoked_at.is_some()
            || context.coordination.team_stale
            || context.coordination.authorization_version != receipt.authorization_version
            || context.coordination.envelope_digest != receipt.envelope_digest
        {
            return Ok(());
        }
        let Some(candidate) = role_fallback_candidates(&context.envelope, &context.task_role_id)
            .into_iter()
            .find(|candidate| {
                candidate.chain_index == receipt.chain_index
                    && candidate.grant.agent_id == receipt.to_actor_id
            })
        else {
            return Ok(());
        };
        if !self
            .fallback_candidate_is_eligible(context, &candidate)
            .await?
        {
            return Ok(());
        }
        let Some(mut task) = TaskRepo::get_by_id(&*self.db, &context.task_id, false).await? else {
            return Ok(());
        };
        let Some(lifecycle) =
            TaskLifecycleRepo::get_task_lifecycle(&*self.db, &context.task_id).await?
        else {
            return Ok(());
        };
        match lifecycle.state {
            db::TaskLifecycleState::Backlog
            | db::TaskLifecycleState::Ready
            | db::TaskLifecycleState::Active => return Ok(()),
            db::TaskLifecycleState::Blocked
                if lifecycle.reason_kind.as_deref() == Some("executor_unavailable")
                    && lifecycle.reason_ref.as_deref()
                        == Some(context.origin_execution_id.as_str()) => {}
            _ => return Ok(()),
        }
        if task.failed_json.is_some()
            || !blocking_metadata_matches(&task, &context.origin_execution_id)
        {
            return Ok(());
        }
        if task.error_annotation.is_some() || task.blocked_json.is_some() {
            task = TaskRepo::update(
                &*self.db,
                db::UpdateTask {
                    id: task.id.clone(),
                    expected_version: task.version,
                    title: None,
                    description: None,
                    priority: None,
                    merge_config: None,
                    error_annotation: Some(None),
                    blocked_json: Some(None),
                    failed_json: None,
                    parent_task_id: None,
                    updated_at: db::now_rfc3339(),
                },
            )
            .await?;
        }
        let result = TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .transition(TransitionLifecycleInput {
                task_id: context.task_id.clone(),
                expected_task_version: task.version,
                to_state: db::TaskLifecycleState::Active,
                cause: LifecycleCause::System(api_types::SystemComponent::TaskDispatcher),
                reason_kind: Some("work_unit_fallback".to_owned()),
                reason_ref: Some(receipt.id.clone()),
                idempotency_key: format!("work-unit-fallback-activate:{}", receipt.id),
            })
            .await?;
        if result.lifecycle.state != db::TaskLifecycleState::Active {
            return Ok(());
        }
        Ok(())
    }

    async fn record_fallback_receipt_event(
        &self,
        context: &FallbackContext,
        receipt: &db::WorkUnitFallbackReceipt,
    ) -> Result<()> {
        let payload = json!({
            "task_id": receipt.task_id,
            "work_unit_id": receipt.work_unit_id,
            "origin_execution_id": receipt.origin_execution_id,
            "receipt_id": receipt.id,
            "task_role_id": receipt.task_role_id,
            "from_actor_id": receipt.from_actor_id,
            "to_actor_id": receipt.to_actor_id,
            "chain_index": receipt.chain_index,
            "cost_class": receipt.cost_class,
            "work_unit_version": receipt.work_unit_version_after,
            "authorization_version": receipt.authorization_version,
            "envelope_digest": receipt.envelope_digest,
            "route_attempts_digest": receipt.route_attempts_digest,
            "outcome": "reallocated",
        });
        self.append_fallback_event(
            context,
            &receipt.origin_execution_id,
            &format!("task.work_unit_fallback:{}", receipt.origin_execution_id),
            payload,
        )
        .await
    }

    async fn ensure_fallback_event(&self, receipt: &db::WorkUnitFallbackReceipt) -> Result<()> {
        if DomainEventRepo::get_event_by_dedupe(
            &*self.db,
            &format!("task.work_unit_fallback:{}", receipt.origin_execution_id),
        )
        .await?
        .is_some()
        {
            return Ok(());
        }
        let task = TaskRepo::get_by_id(&*self.db, &receipt.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", receipt.task_id.clone()))?;
        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, &receipt.task_id)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation("fallback receipt lost its Task coordination")
            })?;
        let context = FallbackContext {
            task_id: receipt.task_id.clone(),
            project_id: task.project_id,
            work_unit_id: receipt.work_unit_id.clone(),
            task_role_id: receipt.task_role_id.clone(),
            origin_execution_id: receipt.origin_execution_id.clone(),
            origin_agent_id: receipt.from_actor_id.clone(),
            work_unit_version: receipt.work_unit_version_before,
            route_attempts_digest: receipt.route_attempts_digest.clone(),
            envelope: serde_json::from_str(&coordination.envelope_json).map_err(|error| {
                ServiceError::invalid_operation(format!("invalid fallback envelope: {error}"))
            })?,
            coordination,
        };
        self.record_fallback_receipt_event(&context, receipt).await
    }

    async fn record_fallback_block(
        &self,
        context: &FallbackContext,
        reason: &str,
        candidate_agent_id: Option<&str>,
        chain_length: i64,
    ) -> Result<()> {
        let payload = json!({
            "task_id": context.task_id,
            "work_unit_id": context.work_unit_id,
            "origin_execution_id": context.origin_execution_id,
            "task_role_id": context.task_role_id,
            "outcome": "blocked",
            "reason": reason,
            "candidate_agent_id": candidate_agent_id,
            "chain_length": chain_length,
            "authorization_version": context.coordination.authorization_version,
            "envelope_digest": context.coordination.envelope_digest,
            "route_attempts_digest": context.route_attempts_digest,
        });
        self.append_fallback_event(
            context,
            &context.origin_execution_id,
            &format!("task.work_unit_fallback:{}", context.origin_execution_id),
            payload,
        )
        .await
    }

    async fn append_fallback_event(
        &self,
        context: &FallbackContext,
        origin_execution_id: &str,
        dedupe_key: &str,
        payload: Value,
    ) -> Result<()> {
        if let Some(existing) = DomainEventRepo::get_event_by_dedupe(&*self.db, dedupe_key).await? {
            let existing_payload: Value = serde_json::from_str(&existing.payload_json)
                .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
            if existing.event_type != "task.work_unit_fallback"
                || existing.entity_type != "work_unit"
                || existing.entity_id != context.work_unit_id
                || existing_payload != payload
            {
                return Err(ServiceError::invalid_operation(
                    "fallback event idempotency key conflicts with a different outcome",
                ));
            }
            DomainEventService::publish_committed_hint(&self.event_bus, &existing);
            return Ok(());
        }
        DomainEventService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .append(CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "task.work_unit_fallback".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: context.work_unit_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: context.task_id.clone(),
                correlation_id: format!("work-unit-fallback:{origin_execution_id}"),
                causation_id: Some(origin_execution_id.to_owned()),
                causation_depth: 1,
                dedupe_key: Some(dedupe_key.to_owned()),
                payload_json: payload.to_string(),
                created_at: db::now_rfc3339(),
            })
            .await?;
        Ok(())
    }

    async fn block_if_needed(
        &self,
        context: &FallbackContext,
        origin_execution_id: &str,
        reason_kind: Option<&str>,
    ) -> Result<()> {
        let Some(task) = TaskRepo::get_by_id(&*self.db, &context.task_id, false).await? else {
            return Ok(());
        };
        let Some(lifecycle) =
            TaskLifecycleRepo::get_task_lifecycle(&*self.db, &context.task_id).await?
        else {
            return Ok(());
        };
        if lifecycle.state == db::TaskLifecycleState::Blocked {
            return Ok(());
        }
        TaskLifecycleService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus))
            .block(
                &task.id,
                LifecycleCause::Execution(origin_execution_id.to_owned()),
                reason_kind.unwrap_or("executor_unavailable"),
                origin_execution_id,
                format!("fallback-block:{}", origin_execution_id),
            )
            .await?;
        Ok(())
    }
}

impl TaskDispatcher {
    async fn fallback_candidate_is_eligible(
        &self,
        context: &FallbackContext,
        candidate: &FallbackCandidate,
    ) -> Result<bool> {
        let Some(project) = ProjectRepo::get_by_id(&*self.db, &context.project_id).await? else {
            return Ok(false);
        };
        let Some(owner_id) = project.owner_id.as_deref() else {
            return Ok(false);
        };
        let Some(origin) = AgentRepo::get_by_id(&*self.db, &context.origin_agent_id).await? else {
            return Ok(false);
        };
        let Some(agent) = AgentRepo::get_by_id(&*self.db, &candidate.grant.agent_id).await? else {
            return Ok(false);
        };
        if origin.owner_id.as_deref() != Some(owner_id)
            || agent.owner_id.as_deref() != Some(owner_id)
            || agent.paused
            || !fallback_actor_is_distinct_and_non_coordinator(
                &agent.id,
                &context.origin_agent_id,
                context.coordination.coordinator_actor.as_ref(),
            )
        {
            return Ok(false);
        }
        let role = TaskRoleRepo::get_by_id(&*self.db, &candidate.grant.task_role_id).await?;
        let work_unit = WorkUnitRepo::get_by_id(&*self.db, &context.work_unit_id).await?;
        if !role.as_ref().is_some_and(|role| {
            role.id == context.task_role_id
                && role.task_id == context.task_id
                && work_unit
                    .as_ref()
                    .is_some_and(|unit| role.role == unit.role)
        }) {
            return Ok(false);
        }
        let active_member: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM role_membership
                WHERE task_role_id = ? AND actor_kind = 'agent'
                  AND actor_id = ? AND status = 'active'
            )",
        )
        .bind(&context.task_role_id)
        .bind(&agent.id)
        .fetch_one(self.db.pool())
        .await?;
        if !active_member {
            return Ok(false);
        }
        if let Some(credential_ref) = agent.credential_ref.as_deref() {
            let credential_available: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM credential_handle handle
                    JOIN protected_credential_secret secret ON secret.handle_id = handle.id
                    WHERE handle.id = ? AND handle.owner_user_id = ?
                      AND handle.status = 'configured'
                )",
            )
            .bind(credential_ref)
            .bind(owner_id)
            .fetch_one(self.db.pool())
            .await?;
            if !credential_available {
                return Ok(false);
            }
        }
        if !crate::work_unit_service::range_review::actor_did_not_contribute(
            &self.db,
            &context.work_unit_id,
            &db::ActorRef::Agent(agent.id.clone()),
        )
        .await?
        {
            return Ok(false);
        }
        let actor = api_types::ActorRef::Agent(agent.id.clone());
        crate::task_rework::reviewer_is_independent(&self.db, &context.work_unit_id, &actor).await
    }
}

fn unavailable_route(execution: &db::Execution) -> Result<Option<UnavailableRoute>> {
    let Some(snapshot) = execution.executor_config_snapshot_json.as_deref() else {
        return Ok(None);
    };
    let Ok(snapshot) = serde_json::from_str::<Value>(snapshot) else {
        return Ok(None);
    };
    let Some(routing) = snapshot.get(executors::ROUTING_SNAPSHOT_KEY) else {
        return Ok(None);
    };
    if routing
        .pointer("/disposition/failure_class")
        .and_then(Value::as_str)
        != Some("executor_unavailable")
    {
        return Ok(None);
    }
    let Some(attempts) = routing.get("attempts").and_then(Value::as_array) else {
        return Ok(None);
    };
    let retry_at = match routing.pointer("/disposition/retry_at") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Ok(None),
    };
    Ok(Some(UnavailableRoute {
        attempts: attempts.clone(),
        retry_at,
    }))
}

fn route_attempts_exhausted(attempts: &[Value]) -> bool {
    !attempts.is_empty()
        && attempts.iter().all(|attempt| {
            attempt
                .get("outcome")
                .and_then(Value::as_str)
                .is_some_and(|outcome| ROUTE_EXHAUSTION_OUTCOMES.contains(&outcome))
        })
}

fn retry_hint_action(
    retry_at: Option<&str>,
    max_wait_seconds: u32,
    now: DateTime<Utc>,
) -> Result<RetryHintAction> {
    let Some(retry_at) = retry_at else {
        return Ok(RetryHintAction::CrossAgent);
    };
    let retry_at = DateTime::parse_from_rfc3339(retry_at)
        .map_err(|error| {
            ServiceError::invalid_operation(format!("invalid executor retry_at: {error}"))
        })?
        .with_timezone(&Utc);
    if retry_at <= now {
        return Ok(RetryHintAction::RetrySameAgent);
    }
    let wait_limit = now + ChronoDuration::seconds(i64::from(max_wait_seconds));
    if retry_at > wait_limit {
        Ok(RetryHintAction::CrossAgent)
    } else {
        Ok(RetryHintAction::Wait)
    }
}

fn role_fallback_candidates(
    envelope: &CoordinationEnvelope,
    task_role_id: &str,
) -> Vec<FallbackCandidate> {
    envelope
        .fallback_agents
        .iter()
        .filter(|candidate| candidate.task_role_id == task_role_id)
        .enumerate()
        .map(|(index, grant)| FallbackCandidate {
            chain_index: i64::try_from(index).unwrap_or(i64::MAX),
            grant: grant.clone(),
        })
        .collect()
}

fn fallback_actor_is_distinct_and_non_coordinator(
    candidate_agent_id: &str,
    origin_agent_id: &str,
    coordinator_actor: Option<&db::ActorRef>,
) -> bool {
    candidate_agent_id != origin_agent_id
        && !coordinator_actor
            .is_some_and(|actor| actor == &db::ActorRef::Agent(candidate_agent_id.to_owned()))
}

async fn execution_has_uncertain_provider_start(
    db: &db::SqliteDb,
    execution_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(
            SELECT 1 FROM domain_event
            WHERE entity_type = 'execution' AND entity_id = ?
              AND event_type = 'execution.provider_start_uncertain'
        )",
    )
    .bind(execution_id)
    .fetch_one(db.pool())
    .await?)
}

fn route_attempts_digest(attempts: &[Value]) -> Result<String> {
    let bytes = serde_json::to_vec(attempts).map_err(|error| {
        ServiceError::invalid_operation(format!("route_attempts could not be serialized: {error}"))
    })?;
    Ok(sha256_hex(&bytes))
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn blocking_metadata_matches(task: &db::Task, execution_id: &str) -> bool {
    let annotation_matches = task.error_annotation.as_deref().is_none_or(|raw| {
        serde_json::from_str::<api_types::TaskBlockingAnnotation>(raw)
            .ok()
            .and_then(|annotation| annotation.blocked_execution_id)
            .as_deref()
            == Some(execution_id)
    });
    let blocked_matches = task.blocked_json.as_deref().is_none_or(|raw| {
        serde_json::from_str::<Value>(raw)
            .ok()
            .and_then(|blocked| {
                blocked
                    .get("execution_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .as_deref()
            == Some(execution_id)
    });
    annotation_matches && blocked_matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn multirepo_workspace_fallback_reuses_same_work_unit_group() {
        use db::{
            AgentRepo, CreateAgent, CreateProject, CreateRepo, CreateTask, CreateTaskRole,
            CreateWorkUnitFallbackReceipt, CreateWorkUnitWorkspace, CreateWorkspace,
            CreateWorkspaceGroup, ProjectRepo, RepoRepo, RoleMembershipRepo, TaskDispatchOperation,
            TaskDispatchState, TaskLifecycleRepo, TaskLifecycleState, TaskRepo, TaskRoleRepo,
            TransitionTaskDispatch, TransitionTaskLifecycle, User, UserRepo,
            WorkUnitFallbackReceiptRepo, WorkUnitWorkspaceRepo, WorkspaceRepo, WorkspaceStatus,
        };

        let pool = db::create_sqlite_pool("sqlite::memory:")
            .await
            .expect("SQLite pool creates");
        db::run_migrations(&pool).await.expect("migrations apply");
        let db = db::SqliteDb::new(pool);
        let now = db::now_rfc3339();
        let owner_id = db::new_uuid_v4();
        UserRepo::create_user(
            &db,
            &User {
                id: owner_id.clone(),
                email: format!("owner-{owner_id}@example.invalid"),
                password_hash: "test-hash".to_owned(),
                display_name: None,
                is_admin: false,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project owner creates");
        let project_id = db::new_uuid_v4();
        ProjectRepo::create(
            &db,
            CreateProject {
                id: project_id.clone(),
                name: "Fallback WorkspaceGroup fixture".to_owned(),
                settings: "{}".to_owned(),
                primary_repo_id: None,
                owner_id: Some(owner_id.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Project creates");
        let repo_id = db::new_uuid_v4();
        RepoRepo::create(
            &db,
            CreateRepo {
                id: repo_id.clone(),
                project_id: project_id.clone(),
                name: "fallback-repo".to_owned(),
                remote_url: "https://example.invalid/fallback.git".to_owned(),
                local_path: None,
                work_mode: db::WorkMode::DirectMerge,
                default_branch: "main".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task repository creates");
        let task_id = db::new_uuid_v4();
        TaskRepo::create(
            &db,
            CreateTask {
                id: task_id.clone(),
                project_id: project_id.clone(),
                repo_id: Some(repo_id.clone()),
                repositories: Vec::new(),
                parent_task_id: None,
                title: "Fallback keeps its WorkspaceGroup".to_owned(),
                description: None,
                lifecycle_state: TaskLifecycleState::Active,
                is_automation: false,
                priority: 0,
                subtask_order: None,
                merge_config: None,
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("Task creates");

        let create_agent = |name: &str| CreateAgent {
            id: db::new_uuid_v4(),
            name: name.to_owned(),
            description: None,
            executor_type: "shell".to_owned(),
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
            status: db::AgentStatus::Idle,
            last_heartbeat_at: None,
            is_default: false,
            paused: false,
            owner_id: Some(owner_id.clone()),
            visibility: "account".to_owned(),
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        let origin = create_agent("Fallback origin");
        let origin_agent_id = origin.id.clone();
        AgentRepo::create(&db, origin)
            .await
            .expect("origin Agent creates");
        let target = create_agent("Fallback target");
        let target_agent_id = target.id.clone();
        AgentRepo::create(&db, target)
            .await
            .expect("target Agent creates");
        let coordinator = create_agent("Fallback coordinator");
        let coordinator_agent_id = coordinator.id.clone();
        AgentRepo::create(&db, coordinator)
            .await
            .expect("coordinator Agent creates");

        let task_role_id = db::new_uuid_v4();
        TaskRoleRepo::create_with_configuration(
            &db,
            CreateTaskRole {
                id: task_role_id.clone(),
                task_id: task_id.clone(),
                role: "implementer".to_owned(),
                coordination_mode: Some(db::CoordinationMode::Collaborative),
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
            r#"{"schema_version":1,"display_label":null,"capabilities":[],"rework":null}"#,
        )
        .await
        .expect("implementer TaskRole creates");
        let coordinator_role_id = db::new_uuid_v4();
        TaskRoleRepo::create(
            &db,
            CreateTaskRole {
                id: coordinator_role_id.clone(),
                task_id: task_id.clone(),
                role: "coordinator".to_owned(),
                coordination_mode: None,
                policy_json: "{}".to_owned(),
                created_at: now.clone(),
                updated_at: now.clone(),
            },
        )
        .await
        .expect("coordinator TaskRole creates");
        for (task_role_id, agent_id) in [
            (&task_role_id, &origin_agent_id),
            (&task_role_id, &target_agent_id),
            (&coordinator_role_id, &coordinator_agent_id),
        ] {
            RoleMembershipRepo::add(
                &db,
                db::CreateRoleMembership {
                    id: db::new_uuid_v4(),
                    task_role_id: task_role_id.clone(),
                    actor_kind: db::ActorKind::Agent,
                    actor_id: agent_id.clone(),
                    status: db::RoleMembershipStatus::Active,
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
            )
            .await
            .expect("Agent joins its exact TaskRole");
        }

        let work_unit_id = db::new_uuid_v4();
        sqlx::query(
            "INSERT INTO work_unit (
                id, task_id, title, scope, status, role, assigned_actor_kind,
                assigned_actor_id, requires_integration, created_by_kind, created_by_id,
                version, created_at, updated_at
             ) VALUES (?, ?, 'implement', 'src/**', 'open', 'implementer', 'agent', ?,
                       1, 'human', ?, 1, ?, ?)",
        )
        .bind(&work_unit_id)
        .bind(&task_id)
        .bind(&origin_agent_id)
        .bind(&owner_id)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("repository WorkUnit creates");
        let group = WorkspaceRepo::create_group_for_work_unit(
            &db,
            CreateWorkspaceGroup {
                id: db::new_uuid_v4(),
                task_id: task_id.clone(),
                work_unit_id: work_unit_id.clone(),
                root_path: format!("/tmp/work-unit-groups/{task_id}/{work_unit_id}"),
                created_at: now.clone(),
            },
        )
        .await
        .expect("exact WorkUnit WorkspaceGroup creates");
        let workspace_id = db::new_uuid_v4();
        let branch = workspace::work_unit_branch_name(&task_id, &work_unit_id, &workspace_id)
            .expect("exact WorkUnit branch derives");
        let worktree_path = format!("{}/{repo_id}", group.root_path);
        let workspace = WorkUnitWorkspaceRepo::create_for_work_unit(
            &db,
            CreateWorkUnitWorkspace {
                workspace: CreateWorkspace {
                    id: workspace_id.clone(),
                    task_id: task_id.clone(),
                    repo_id: repo_id.clone(),
                    worktree_path,
                    branch,
                    status: WorkspaceStatus::Ready,
                    before_sha: Some("0123456789abcdef0123456789abcdef01234567".to_owned()),
                    created_at: now.clone(),
                    updated_at: now.clone(),
                },
                work_unit_id: work_unit_id.clone(),
                workspace_group_id: group.id.clone(),
            },
        )
        .await
        .expect("exact WorkUnit repository Workspace creates");

        let origin_execution_id = db::new_uuid_v4();
        sqlx::query("DROP TRIGGER work_unit_execution_admission_guard")
            .execute(db.pool())
            .await
            .expect("fixture skips only live Workspace admission checks");
        sqlx::query(
            "INSERT INTO execution (
                id, task_id, agent_id, actor_kind, actor_id, role, status,
                work_unit_id, work_unit_version, created_at, updated_at
             ) VALUES (?, ?, ?, 'agent', ?, 'implementer', 'failed', ?, 1, ?, ?)",
        )
        .bind(&origin_execution_id)
        .bind(&task_id)
        .bind(&origin_agent_id)
        .bind(&origin_agent_id)
        .bind(&work_unit_id)
        .bind(&now)
        .bind(&now)
        .execute(db.pool())
        .await
        .expect("exact failed origin Execution creates");
        let envelope = json!({
            "schema_version": 2,
            "fallback_agents": [{
                "task_role_id": task_role_id,
                "agent_id": target_agent_id,
                "cost_class": "economy",
                "max_wait_seconds": 30
            }]
        });
        let envelope_json = envelope.to_string();
        let envelope_digest = sha256_hex(envelope_json.as_bytes());
        sqlx::query(
            "UPDATE task_coordination SET mode = 'prepare_and_execute',
                coordinator_task_role_id = ?, coordinator_actor_kind = 'agent',
                coordinator_actor_id = ?, authorization_version = 1,
                envelope_json = ?, envelope_digest = ?, requested_by = ?, team_stale = 0
             WHERE task_id = ?",
        )
        .bind(&coordinator_role_id)
        .bind(&coordinator_agent_id)
        .bind(&envelope_json)
        .bind(&envelope_digest)
        .bind(&owner_id)
        .bind(&task_id)
        .execute(db.pool())
        .await
        .expect("current fallback authorization installs");

        let task = TaskRepo::get_by_id(&db, &task_id, false)
            .await
            .expect("Task loads")
            .expect("Task exists");
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&db, &task_id)
            .await
            .expect("TaskLifecycle loads")
            .expect("TaskLifecycle exists");
        let lifecycle_now = db::now_rfc3339();
        TaskLifecycleRepo::transition_task_lifecycle(
            &db,
            TransitionTaskLifecycle {
                id: db::new_uuid_v4(),
                task_id: task_id.clone(),
                expected_task_version: task.version,
                expected_lifecycle_version: lifecycle.version,
                expected_state: TaskLifecycleState::Active,
                to_state: TaskLifecycleState::Blocked,
                cause_kind: "execution".to_owned(),
                cause_ref: Some(origin_execution_id.clone()),
                reason_kind: Some("executor_unavailable".to_owned()),
                reason_ref: Some(origin_execution_id.clone()),
                gate_evaluation_id: None,
                idempotency_key: "multirepo-fallback-block".to_owned(),
                updated_at: lifecycle_now.clone(),
                event: db::CreateDomainEvent {
                    id: db::new_uuid_v4(),
                    event_type: "task.lifecycle_changed".to_owned(),
                    entity_type: "task".to_owned(),
                    entity_id: task_id.clone(),
                    actor_type: "system".to_owned(),
                    actor_id: None,
                    scope_type: "task".to_owned(),
                    scope_id: task_id.clone(),
                    correlation_id: "multirepo-fallback-block".to_owned(),
                    causation_id: Some(origin_execution_id.clone()),
                    causation_depth: 1,
                    dedupe_key: Some(format!("task-lifecycle:{task_id}:multirepo-fallback-block")),
                    payload_json: json!({"task_id":task_id,"to_state":"blocked"}).to_string(),
                    created_at: lifecycle_now.clone(),
                },
            },
        )
        .await
        .expect("origin Execution owns the exact fallback block");
        TaskLifecycleRepo::transition_task_dispatch(
            &db,
            TransitionTaskDispatch {
                task_id: task_id.clone(),
                operation: TaskDispatchOperation::Launch,
                state: TaskDispatchState::Running,
                expected_intent_version: 1,
                requested_by_kind: "system".to_owned(),
                requested_by_id: None,
                reason: None,
                idempotency_key: "multirepo-fallback-launch".to_owned(),
                request_digest: sha256_hex(b"multirepo-fallback-launch"),
                receipt_id: db::new_uuid_v4(),
                created_at: lifecycle_now,
            },
        )
        .await
        .expect("Task dispatch resumes for the exact Task");

        let receipt = CreateWorkUnitFallbackReceipt {
            id: db::new_uuid_v4(),
            task_id: task_id.clone(),
            work_unit_id: work_unit_id.clone(),
            work_unit_version_before: 1,
            work_unit_version_after: 2,
            origin_execution_id: origin_execution_id.clone(),
            failure_class: "executor_unavailable".to_owned(),
            route_attempts_digest: sha256_hex(b"exhausted route"),
            task_role_id,
            from_actor_kind: db::ActorKind::Agent,
            from_actor_id: origin_agent_id,
            to_actor_kind: db::ActorKind::Agent,
            to_actor_id: target_agent_id.clone(),
            chain_index: 0,
            cost_class: db::CostClass::Economy,
            authorization_version: 1,
            envelope_digest,
            new_execution_id: None,
            created_at: db::now_rfc3339(),
        };
        let (_, _) = WorkUnitFallbackReceiptRepo::allocate_work_unit_with_fallback_receipt(
            &db,
            db::AllocateWorkUnit {
                id: work_unit_id.clone(),
                expected_version: 1,
                role: "implementer".to_owned(),
                assigned_actor: Some(db::ActorRef::Agent(target_agent_id.clone())),
                updated_at: db::now_rfc3339(),
            },
            db::CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "work_unit.allocation_changed".to_owned(),
                entity_type: "work_unit".to_owned(),
                entity_id: work_unit_id.clone(),
                actor_type: "system".to_owned(),
                actor_id: None,
                scope_type: "task".to_owned(),
                scope_id: task_id.clone(),
                correlation_id: "multirepo-fallback-reallocation".to_owned(),
                causation_id: Some(origin_execution_id),
                causation_depth: 1,
                dedupe_key: Some("multirepo-fallback-reallocation".to_owned()),
                payload_json:
                    json!({"work_unit_id":work_unit_id,"fallback_agent_id":target_agent_id})
                        .to_string(),
                created_at: db::now_rfc3339(),
            },
            receipt,
        )
        .await
        .expect("fallback atomically reallocates the existing WorkUnit");
        let replayed_group = WorkspaceRepo::get_group_by_work_unit_id(&db, &work_unit_id)
            .await
            .expect("WorkspaceGroup lookup succeeds")
            .expect("fallback keeps the existing WorkspaceGroup");
        assert_eq!(replayed_group.id, group.id);
        assert_eq!(replayed_group.root_path, group.root_path);
        let replayed_workspaces = WorkUnitWorkspaceRepo::list_by_work_unit_id(&db, &work_unit_id)
            .await
            .expect("reallocated WorkUnit repository workspaces load");
        assert_eq!(replayed_workspaces.len(), 1);
        assert_eq!(replayed_workspaces[0].id, workspace.id);
        assert_eq!(
            replayed_workspaces[0].worktree_path,
            workspace.worktree_path
        );
        let work_unit = db::WorkUnitRepo::get_by_id(&db, &work_unit_id)
            .await
            .expect("WorkUnit lookup succeeds")
            .expect("reallocated WorkUnit exists");
        assert_eq!(work_unit.id, group.work_unit_id);
        assert_eq!(
            work_unit.assigned_actor,
            Some(db::ActorRef::Agent(target_agent_id))
        );
    }

    fn route_attempt(outcome: &str) -> Value {
        json!({"candidate_key": "candidate", "outcome": outcome})
    }

    fn grant(
        task_role_id: &str,
        agent_id: &str,
    ) -> crate::task_coordination::EnvelopeFallbackAgent {
        crate::task_coordination::EnvelopeFallbackAgent {
            task_role_id: task_role_id.to_owned(),
            agent_id: agent_id.to_owned(),
            cost_class: db::CostClass::Economy,
            max_wait_seconds: 30,
        }
    }

    #[test]
    fn fallback_route_requires_a_nonempty_exhausted_route() {
        assert!(!route_attempts_exhausted(&[]));
        assert!(route_attempts_exhausted(&[
            route_attempt("usage_exhausted"),
            route_attempt("skipped_cooldown"),
            route_attempt("unavailable"),
        ]));
        assert!(!route_attempts_exhausted(&[
            route_attempt("unavailable"),
            route_attempt("failed"),
        ]));
    }

    #[test]
    fn fallback_retry_hint_waits_or_retries_the_same_agent_within_budget() {
        let now = DateTime::parse_from_rfc3339("2026-11-17T12:00:00Z")
            .expect("test time parses")
            .with_timezone(&Utc);
        assert_eq!(
            retry_hint_action(Some("2026-11-17T12:00:20Z"), 30, now)
                .expect("short retry hint parses"),
            RetryHintAction::Wait
        );
        assert_eq!(
            retry_hint_action(Some("2026-11-17T11:59:59Z"), 30, now)
                .expect("due retry hint parses"),
            RetryHintAction::RetrySameAgent
        );
        assert_eq!(
            retry_hint_action(Some("2026-11-17T12:00:31Z"), 30, now)
                .expect("long retry hint parses"),
            RetryHintAction::CrossAgent
        );
        assert_eq!(
            retry_hint_action(None, 30, now).expect("missing retry hint is allowed"),
            RetryHintAction::CrossAgent
        );
    }

    #[test]
    fn fallback_chain_is_ordered_and_scoped_to_the_exact_task_role() {
        let envelope = CoordinationEnvelope {
            schema_version: 2,
            fallback_agents: vec![
                grant("role-a", "agent-1"),
                grant("role-b", "agent-2"),
                grant("role-a", "agent-3"),
            ],
            ..CoordinationEnvelope::default()
        };
        let candidates = role_fallback_candidates(&envelope, "role-a");
        assert_eq!(
            candidates
                .iter()
                .map(|candidate| (candidate.chain_index, candidate.grant.agent_id.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "agent-1"), (1, "agent-3")]
        );
        assert!(role_fallback_candidates(&envelope, "missing").is_empty());
    }

    #[test]
    fn fallback_never_selects_the_origin_or_coordinator_agent() {
        let coordinator = db::ActorRef::Agent("coordinator".to_owned());
        assert!(!fallback_actor_is_distinct_and_non_coordinator(
            "origin",
            "origin",
            Some(&coordinator)
        ));
        assert!(!fallback_actor_is_distinct_and_non_coordinator(
            "coordinator",
            "origin",
            Some(&coordinator)
        ));
        assert!(fallback_actor_is_distinct_and_non_coordinator(
            "fallback",
            "origin",
            Some(&coordinator)
        ));
    }

    #[test]
    fn fallback_receipt_route_digest_is_stable_and_order_sensitive() {
        let first = vec![
            route_attempt("unavailable"),
            route_attempt("usage_exhausted"),
        ];
        let reversed = vec![
            route_attempt("usage_exhausted"),
            route_attempt("unavailable"),
        ];
        assert_eq!(
            route_attempts_digest(&first).expect("route digest computes"),
            route_attempts_digest(&first).expect("same route digest computes")
        );
        assert_ne!(
            route_attempts_digest(&first).expect("route digest computes"),
            route_attempts_digest(&reversed).expect("reversed route digest computes")
        );
    }

    #[test]
    fn fallback_block_metadata_must_bind_the_exact_execution() {
        let mut task = test_task();
        task.error_annotation = Some(
            serde_json::to_string(&api_types::TaskBlockingAnnotation {
                annotation_type: api_types::FailureKind::ExecutorUnavailable,
                blocking_reason: "executor_unavailable".to_owned(),
                blocked_by: None,
                blocked_at: None,
                blocked_execution_id: Some("execution-1".to_owned()),
                artifact: None,
                message: None,
                hook: None,
                recovery_actions: Vec::new(),
            })
            .expect("blocking annotation serializes"),
        );
        task.blocked_json = Some(json!({"execution_id":"execution-1"}).to_string());
        assert!(blocking_metadata_matches(&task, "execution-1"));
        assert!(!blocking_metadata_matches(&task, "execution-2"));
    }

    fn test_task() -> db::Task {
        db::Task {
            id: "task".to_owned(),
            project_id: "project".to_owned(),
            repo_id: None,
            parent_task_id: None,
            title: "fallback task".to_owned(),
            description: None,
            lifecycle_state: db::TaskLifecycleState::Blocked,
            is_automation: false,
            priority: 0,
            board_position: 0.0,
            subtask_order: None,
            merge_config: None,
            metadata_json: None,
            error_annotation: None,
            blocked_json: None,
            failed_json: None,
            deleted_at: None,
            version: 1,
            created_at: "2026-11-17T00:00:00Z".to_owned(),
            updated_at: "2026-11-17T00:00:00Z".to_owned(),
        }
    }
}
