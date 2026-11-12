use std::sync::Arc;

use db::{
    ActorRef, TaskCoordinationMode, TaskCoordinationRepo, TaskLifecycleRepo, TaskLifecycleState,
    TaskRepo,
};
use serde_json::{json, Value};

use super::scheduler::{
    batch_projection_by_id_in_tx, batch_projection_in_tx, coordination_blocker,
    project_admin_current_in_tx,
};
use super::{
    authorize_project_admin_in_tx, decode, digest, ensure_task_in_tx, invalid, put_receipt,
    receipt, types::BatchProjection, ProjectPlanBatch, ProjectPlanService,
};
use crate::{
    task_coordination::{AuthorizationInput, CoordinationEnvelope, TaskCoordinationService},
    Result, ServiceError,
};

const MAX_BATCH_TASKS: usize = 1000;

impl ProjectPlanService {
    pub async fn authorize_batch(
        &self,
        project_id: &str,
        user_id: &str,
        expected_version: i64,
        task_ids: Vec<String>,
        max_parallel_tasks: i64,
        idempotency_key: &str,
    ) -> Result<ProjectPlanBatch> {
        let batch = self
            .create_batch_record(
                project_id,
                user_id,
                expected_version,
                task_ids,
                max_parallel_tasks,
                idempotency_key,
            )
            .await?;
        self.prepare_batch(&batch.id).await?;
        self.load_batch(&batch.id).await
    }

    /// Create the durable, explicitly authorized batch and capture its exact
    /// Task authority baselines, but leave coordinator preparation pending.
    /// This seam is also used by service tests that need to exercise failures
    /// between the batch grant and coordinator pin without fabricating SQL
    /// rows or invoking the whole prepare-and-run path.
    pub(super) async fn create_batch_record(
        &self,
        project_id: &str,
        user_id: &str,
        expected_version: i64,
        mut task_ids: Vec<String>,
        max_parallel_tasks: i64,
        idempotency_key: &str,
    ) -> Result<ProjectPlanBatch> {
        super::key_valid(idempotency_key)?;
        if task_ids.is_empty()
            || task_ids.len() > MAX_BATCH_TASKS
            || !(1..=64).contains(&max_parallel_tasks)
        {
            return Err(invalid(
                "A batch requires 1 to 1000 Tasks and max_parallel_tasks from 1 to 64",
            ));
        }
        let original_len = task_ids.len();
        task_ids.sort();
        task_ids.dedup();
        if task_ids.len() != original_len || task_ids.iter().any(|id| id.trim().is_empty()) {
            return Err(invalid("Batch Task IDs must be nonempty and unique"));
        }
        let request_digest = digest(&(
            "authorize_batch",
            user_id,
            expected_version,
            &task_ids,
            max_parallel_tasks,
        ))?;
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        authorize_project_admin_in_tx(&mut tx, project_id, user_id).await?;
        if let Some((stored, result)) = receipt(&mut tx, project_id, idempotency_key).await? {
            if stored != request_digest {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            let result: Value = decode(&result)?;
            let batch_id = result
                .get("batch_id")
                .and_then(Value::as_str)
                .ok_or_else(|| invalid("Batch authorization receipt has no batch ID"))?
                .to_owned();
            tx.commit().await?;
            return self.load_batch(&batch_id).await;
        }
        self.version_in_tx(&mut tx, project_id, expected_version)
            .await?;
        for task_id in &task_ids {
            ensure_task_in_tx(&mut tx, project_id, task_id).await?;
            let reserved: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_plan_batch_task bt
                    JOIN project_plan_batch b ON b.id=bt.batch_id
                    WHERE bt.task_id=? AND b.project_id=? AND b.state IN ('running','paused')
                )",
            )
            .bind(task_id)
            .bind(project_id)
            .fetch_one(&mut *tx)
            .await?;
            if reserved {
                return Err(ServiceError::Conflict(format!(
                    "Task {task_id} is already reserved by an active Project batch"
                )));
            }
        }
        let now = db::now_rfc3339();
        let batch_id = db::new_uuid_v4();
        sqlx::query(
            "INSERT INTO project_plan_batch(
                id,project_id,graph_version,version,state,preparation_complete,
                max_parallel_tasks,authorized_by,idempotency_key,created_at,updated_at
             ) VALUES(?,?,?,1,'paused',0,?,?,?,?,?)",
        )
        .bind(&batch_id)
        .bind(project_id)
        .bind(expected_version)
        .bind(max_parallel_tasks)
        .bind(user_id)
        .bind(idempotency_key)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
        for task_id in &task_ids {
            super::scheduler::insert_batch_task_with_authority_baseline_in_tx(
                &mut tx, &batch_id, task_id,
            )
            .await?;
        }
        put_receipt(
            &mut tx,
            project_id,
            idempotency_key,
            &request_digest,
            &json!({"batch_id":batch_id}),
        )
        .await?;
        tx.commit().await?;
        self.load_batch(&batch_id).await
    }

    pub async fn update_batch(
        &self,
        project_id: &str,
        user_id: &str,
        batch_id: &str,
        expected_version: i64,
        action: &str,
        idempotency_key: &str,
    ) -> Result<ProjectPlanBatch> {
        super::key_valid(idempotency_key)?;
        if !matches!(action, "pause" | "resume" | "revoke") {
            return Err(invalid("Batch action must be pause, resume, or revoke"));
        }
        let request_digest =
            digest(&("update_batch", user_id, batch_id, expected_version, action))?;
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        authorize_project_admin_in_tx(&mut tx, project_id, user_id).await?;
        if let Some((stored, result)) = receipt(&mut tx, project_id, idempotency_key).await? {
            if stored != request_digest {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            let result: Value = decode(&result)?;
            if result.get("batch_id").and_then(Value::as_str) != Some(batch_id) {
                return Err(db::DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return self.load_batch(batch_id).await;
        }
        let batch = batch_projection_in_tx(&mut tx, project_id, batch_id).await?;
        if batch.version != expected_version {
            return Err(db::DbError::VersionConflict.into());
        }
        let actual_graph_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(project_id)
                .fetch_one(&mut *tx)
                .await?;
        let mut next_state = batch.state.clone();
        match action {
            "pause" if batch.state == "running" => next_state = "paused".into(),
            "pause" if batch.state == "paused" => (),
            "resume" if batch.state == "paused" && !batch.preparation_complete => {
                return Err(ServiceError::Conflict(
                    "Batch coordinator preparation is incomplete".into(),
                ));
            }
            "resume" if batch.state == "paused" && batch.graph_version != actual_graph_version => {
                return Err(db::DbError::VersionConflict.into());
            }
            "resume" if batch.state == "paused" => next_state = "running".into(),
            "revoke" if matches!(batch.state.as_str(), "running" | "paused") => {
                next_state = "revoked".into()
            }
            _ if batch.state == "revoked" && action == "revoke" => (),
            _ => {
                return Err(ServiceError::Conflict(format!(
                    "Cannot {action} a Project batch in state {}",
                    batch.state
                )));
            }
        }
        let changed = next_state != batch.state || !batch.preparation_complete;
        let next_version = if changed {
            batch.version + 1
        } else {
            batch.version
        };
        let now = db::now_rfc3339();
        if changed {
            sqlx::query("UPDATE project_plan_batch SET state=?,preparation_complete=1,version=version+1,updated_at=? WHERE id=?")
                .bind(&next_state).bind(&now).bind(batch_id).execute(&mut *tx).await?;
        }
        if action == "revoke" {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker='batch:revoked' WHERE batch_id=? AND state IN ('pending','claimed','started','blocked')")
                .bind(batch_id).execute(&mut *tx).await?;
        }
        put_receipt(
            &mut tx,
            project_id,
            idempotency_key,
            &request_digest,
            &json!({"batch_id":batch_id,"version":next_version,"state":next_state}),
        )
        .await?;
        let event = if changed {
            Some(self.event_with_payload(&mut tx,project_id,user_id,"project.plan_batch_updated",batch_id,json!({"project_id":project_id,"batch_id":batch_id,"graph_version":batch.graph_version,"version":next_version,"state":next_state})).await?)
        } else {
            None
        };
        tx.commit().await?;
        if let Some(event) = event {
            self.publish(&event);
        }
        self.load_batch(batch_id).await
    }

    pub(super) async fn prepare_batch(&self, batch_id: &str) -> Result<()> {
        let initial = self.load_batch_projection(batch_id).await?;
        if initial.preparation_complete || initial.state != "paused" {
            return Ok(());
        }

        for row in self.batch_task_rows(batch_id).await? {
            if row.state == "completed"
                || row.state == "started"
                || row.coordination_request_id.is_some()
            {
                continue;
            }
            if self
                .preparation_blocker(batch_id, &row.task_id)
                .await?
                .is_some()
            {
                break;
            }
            let (state, blocker) =
                match self.prepare_task_coordination(&initial, &row.task_id).await {
                    Ok(TaskPreparation::Ready) => ("pending", None),
                    Ok(TaskPreparation::Completed) => ("completed", None),
                    Err(blocker) => ("blocked", Some(blocker)),
                };
            if !self
                .record_preparation_result(batch_id, &row.task_id, state, blocker.as_deref())
                .await?
            {
                break;
            }
        }

        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let current = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        if current.state != "paused" || current.preparation_complete {
            tx.commit().await?;
            return Ok(());
        }
        let actual_graph_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&current.project_id)
                .fetch_one(&mut *tx)
                .await?;
        let author_current =
            project_admin_current_in_tx(&mut tx, &current.project_id, &current.authorized_by)
                .await?;
        let graph_current = current.graph_version == actual_graph_version;
        if !graph_current {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker='plan:graph_version_changed' WHERE batch_id=? AND state IN ('pending','claimed')")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        } else if !author_current {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker='batch:authorization_revoked' WHERE batch_id=? AND state IN ('pending','claimed')")
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        }
        if current.version == initial.version && graph_current && author_current {
            sqlx::query("UPDATE project_plan_batch SET state='running',preparation_complete=1,version=version+1,updated_at=? WHERE id=?")
            .bind(db::now_rfc3339())
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        } else {
            sqlx::query("UPDATE project_plan_batch SET preparation_complete=1,version=version+1,updated_at=? WHERE id=?")
            .bind(db::now_rfc3339())
            .bind(batch_id)
            .execute(&mut *tx)
            .await?;
        }
        let final_batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        let event = if matches!(final_batch.state.as_str(), "running" | "paused") {
            Some(
                self.event_with_payload(
                    &mut tx,
                    &current.project_id,
                    &current.authorized_by,
                    "project.plan_batch_authorized",
                    batch_id,
                    json!({
                        "project_id": current.project_id,
                        "batch_id": batch_id,
                        "graph_version": final_batch.graph_version,
                        "version": final_batch.version,
                        "state": final_batch.state
                    }),
                )
                .await?,
            )
        } else {
            None
        };
        tx.commit().await?;
        if let Some(event) = event {
            self.publish(&event);
        }
        Ok(())
    }

    /// Retry one Project task that was lifecycle-blocked at batch grant after
    /// a Human has moved it back into a schedulable state. The batch's pinned
    /// Task, Team, envelope, coordinator, and objective baseline still governs
    /// this retry; any drift leaves the Task blocked with a concrete reason.
    pub(super) async fn prepare_waiting_lifecycle_task(
        &self,
        batch_id: &str,
        task_id: &str,
    ) -> Result<bool> {
        let batch = self.load_batch_projection(batch_id).await?;
        if batch.state != "running" || !batch.preparation_complete {
            return Ok(false);
        }
        let task_row = self
            .batch_task_rows(batch_id)
            .await?
            .into_iter()
            .find(|row| row.task_id == task_id);
        let Some(task_row) = task_row else {
            return Ok(false);
        };
        if task_row.state != "blocked" || task_row.blocker.as_deref() != Some("lifecycle:blocked") {
            return Ok(false);
        }
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id).await?;
        if !lifecycle.is_some_and(|lifecycle| {
            matches!(
                lifecycle.state,
                TaskLifecycleState::Backlog | TaskLifecycleState::Ready
            )
        }) {
            return Ok(false);
        }

        let (state, blocker) = match self.prepare_task_coordination(&batch, task_id).await {
            Ok(TaskPreparation::Ready) => ("pending", None),
            Ok(TaskPreparation::Completed) => ("completed", None),
            Err(blocker) => ("blocked", Some(blocker)),
        };
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let current = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        if current.state != "running" || !current.preparation_complete {
            tx.commit().await?;
            return Ok(true);
        }
        let graph_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&current.project_id)
                .fetch_one(&mut *tx)
                .await?;
        let author_current =
            project_admin_current_in_tx(&mut tx, &current.project_id, &current.authorized_by)
                .await?;
        if graph_version != current.graph_version || !author_current {
            tx.commit().await?;
            return Ok(true);
        }
        sqlx::query(
            "UPDATE project_plan_batch_task SET state=?,claim_expires_at=NULL,blocker=?
             WHERE batch_id=? AND task_id=? AND state='blocked' AND blocker='lifecycle:blocked'",
        )
        .bind(state)
        .bind(blocker)
        .bind(batch_id)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn preparation_blocker(&self, batch_id: &str, task_id: &str) -> Result<Option<String>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        let blocker = if batch.state != "paused" || batch.preparation_complete {
            Some(if batch.state == "revoked" {
                "batch:revoked".to_owned()
            } else {
                "batch:inactive".to_owned()
            })
        } else {
            let graph_version: i64 =
                sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                    .bind(&batch.project_id)
                    .fetch_one(&mut *tx)
                    .await?;
            if graph_version != batch.graph_version {
                Some("plan:graph_version_changed".to_owned())
            } else if !project_admin_current_in_tx(&mut tx, &batch.project_id, &batch.authorized_by)
                .await?
            {
                Some("batch:authorization_revoked".to_owned())
            } else {
                let task_is_in_scope: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                    SELECT 1 FROM project_plan_batch_task bt
                    JOIN task t ON t.id=bt.task_id
                    WHERE bt.batch_id=? AND bt.task_id=?
                      AND t.project_id=? AND t.deleted_at IS NULL
                )",
                )
                .bind(batch_id)
                .bind(task_id)
                .bind(&batch.project_id)
                .fetch_one(&mut *tx)
                .await?;
                (!task_is_in_scope).then(|| "coordination:task_scope_changed".to_owned())
            }
        };
        tx.commit().await?;
        Ok(blocker)
    }

    async fn record_preparation_result(
        &self,
        batch_id: &str,
        task_id: &str,
        state: &str,
        blocker: Option<&str>,
    ) -> Result<bool> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        if batch.state != "paused" || batch.preparation_complete {
            tx.commit().await?;
            return Ok(false);
        }
        let graph_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&batch.project_id)
                .fetch_one(&mut *tx)
                .await?;
        let author_current =
            project_admin_current_in_tx(&mut tx, &batch.project_id, &batch.authorized_by).await?;
        if graph_version != batch.graph_version || !author_current {
            tx.commit().await?;
            return Ok(false);
        }
        let update = sqlx::query(
            "UPDATE project_plan_batch_task SET state=?,claim_expires_at=NULL,blocker=?
         WHERE batch_id=? AND task_id=? AND state IN ('pending','blocked')",
        )
        .bind(state)
        .bind(blocker)
        .bind(batch_id)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(update.rows_affected() == 1)
    }

    async fn prepare_task_coordination(
        &self,
        batch: &BatchProjection,
        task_id: &str,
    ) -> std::result::Result<TaskPreparation, String> {
        let baseline = self
            .read_batch_task_authority_baseline(&batch.id, task_id)
            .await
            .map_err(|_| "coordination:authority_baseline_unavailable".to_owned())?;
        let task = TaskRepo::get_by_id(&*self.db, task_id, false)
            .await
            .map_err(|_| "coordination:task_unavailable".to_owned())?
            .ok_or_else(|| "coordination:task_unavailable".to_owned())?;
        let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, task_id)
            .await
            .map_err(|_| "coordination:lifecycle_unavailable".to_owned())?
            .ok_or_else(|| "coordination:lifecycle_unavailable".to_owned())?;
        let objective_digest = self
            .batch_task_objective_digest(task_id)
            .await
            .map_err(|_| "coordination:task_objective_unavailable".to_owned())?;
        if objective_digest != baseline.task_objective_digest {
            return Err("coordination:task_objective_changed_since_batch_grant".to_owned());
        }
        if lifecycle.state == TaskLifecycleState::Done {
            if self
                .task_delivery_complete(task_id)
                .await
                .map_err(|_| "delivery:acceptance_unavailable".to_owned())?
            {
                return Ok(TaskPreparation::Completed);
            }
            return Err("lifecycle:done".to_owned());
        }
        if !matches!(
            lifecycle.state,
            TaskLifecycleState::Backlog | TaskLifecycleState::Ready
        ) {
            return Err(format!("lifecycle:{}", lifecycle.state));
        }
        let lifecycle_and_version_match = if baseline.lifecycle_state == "blocked" {
            baseline.task_version.checked_add(1) == Some(task.version)
                && matches!(
                    lifecycle.state,
                    TaskLifecycleState::Backlog | TaskLifecycleState::Ready
                )
        } else {
            matches!(baseline.lifecycle_state.as_str(), "backlog" | "ready")
                && baseline.task_version == task.version
        };
        if !lifecycle_and_version_match {
            return Err("coordination:task_changed_since_batch_grant".to_owned());
        }
        if baseline.has_open_work_units
            || baseline.has_non_cancelled_work_units
            || baseline.has_approved_or_materialized_plan
        {
            return Err("coordination:work_already_planned".to_owned());
        }
        let accepted_work: bool = sqlx::query_scalar(
            "SELECT EXISTS(
            SELECT 1 FROM task_coordination_plan
            WHERE task_id=? AND state IN ('approved','materialized')
        ) OR EXISTS(
                SELECT 1 FROM work_unit WHERE task_id=? AND status!='cancelled'
        )",
        )
        .bind(task_id)
        .bind(task_id)
        .fetch_one(self.db.pool())
        .await
        .map_err(|_| "coordination:work_already_planned".to_owned())?;
        if accepted_work {
            return Err("coordination:work_already_planned".to_owned());
        }
        let coordination = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id)
            .await
            .map_err(|_| "coordination:unavailable".to_owned())?
            .ok_or_else(|| "coordination:unconfigured".to_owned())?;
        let Some(expected_authorization_version) = baseline.authorization_version else {
            return Err("coordination:unconfigured".to_owned());
        };
        let Some(expected_team_generation) = baseline.team_generation else {
            return Err("coordination:unconfigured".to_owned());
        };
        let unconfigured_default = batch_baseline_is_unconfigured_default(&baseline);
        if (baseline.team_stale != Some(false) && !unconfigured_default)
            || baseline.revoked_at.is_some()
        {
            return Err("coordination:authorization_stale".to_owned());
        }
        let Some(role_id) = baseline.role_id.clone() else {
            return Err("coordination:coordinator_role_missing".into());
        };
        let Some(actor_id) = baseline.actor_id.clone() else {
            return Err("coordination:coordinator_actor_missing".into());
        };
        if baseline.actor_kind.as_deref() != Some("agent") {
            return Err("coordination:agent_required".into());
        }
        let actor = ActorRef::Agent(actor_id);
        let envelope = batch_envelope_input(baseline)?;
        let mut has_exact_batch_authorization =
            TaskCoordinationRepo::has_project_plan_batch_coordination_authorization(
                &*self.db,
                &batch.id,
                task_id,
                coordination.authorization_version,
                baseline.batch_version,
                batch.graph_version,
            )
            .await
            .map_err(|_| "coordination:authorization_scope_unavailable".to_owned())?;
        // A lifecycle-blocked Task can be re-prepared after the original batch
        // preparation completed. If its exact batch grant was not minted during
        // the original prepare, it is minted against the current batch version;
        // a crash retry must find that durable receipt too.
        if !has_exact_batch_authorization && batch.version != baseline.batch_version {
            has_exact_batch_authorization =
                TaskCoordinationRepo::has_project_plan_batch_coordination_authorization(
                    &*self.db,
                    &batch.id,
                    task_id,
                    coordination.authorization_version,
                    batch.version,
                    batch.graph_version,
                )
                .await
                .map_err(|_| "coordination:authorization_scope_unavailable".to_owned())?;
        }
        let baseline_unchanged = coordination.authorization_version
            == expected_authorization_version
            && coordination.mode.to_string() == baseline.mode.as_deref().unwrap_or_default()
            && coordination.team_generation == expected_team_generation
            && coordination.team_digest == baseline.team_digest.as_deref().unwrap_or_default()
            && coordination.envelope_digest
                == baseline.envelope_digest.as_deref().unwrap_or_default()
            && coordination.requested_by == baseline.requested_by
            && coordination.coordinator_task_role_id == baseline.role_id
            && coordination
                .coordinator_actor
                .as_ref()
                .map(ActorRef::kind)
                .map(|kind| kind.to_string())
                == baseline.actor_kind
            && coordination
                .coordinator_actor
                .as_ref()
                .map(|actor| actor.id())
                == baseline.actor_id.as_deref()
            && coordination.revoked_at == baseline.revoked_at
            && coordination.team_stale == baseline.team_stale.unwrap_or(true);
        if has_exact_batch_authorization {
            self.pin_batch_coordination_snapshot(
                &batch.id,
                task_id,
                &batch.authorized_by,
                task.version,
                coordination.authorization_version,
                coordination.team_generation,
            )
            .await
            .map_err(|error| coordination_blocker(&error))?;
            return Ok(TaskPreparation::Ready);
        }
        if !baseline_unchanged {
            return Err("coordination:authorization_changed_since_batch_grant".to_owned());
        }
        let service =
            TaskCoordinationService::new(Arc::clone(&self.db), Arc::clone(&self.event_bus));
        let key = format!("project-plan-batch:{}:coordination:{}", batch.id, task_id);
        let expected_batch_version = if baseline.lifecycle_state == "blocked" {
            batch.version
        } else {
            baseline.batch_version
        };
        match service
            .authorize_for_project_plan_batch(
                task_id,
                &batch.authorized_by,
                AuthorizationInput {
                    expected_task_version: task.version,
                    expected_coordination_version: expected_authorization_version,
                    mode: TaskCoordinationMode::PrepareAndExecute,
                    coordinator_task_role_id: Some(role_id),
                    coordinator_actor: Some(actor),
                    envelope,
                    idempotency_key: key,
                },
                db::ProjectPlanBatchAuthorizationBinding {
                    project_id: batch.project_id.clone(),
                    batch_id: batch.id.clone(),
                    authorized_by: batch.authorized_by.clone(),
                    expected_batch_version,
                    expected_graph_version: batch.graph_version,
                    expected_team_generation,
                },
            )
            .await
        {
            Ok(overview) => {
                self.pin_batch_coordination_snapshot(
                    &batch.id,
                    task_id,
                    &batch.authorized_by,
                    overview.task.version,
                    overview.coordination.authorization_version,
                    overview.coordination.team_generation,
                )
                .await
                .map_err(|error| coordination_blocker(&error))?;
                Ok(TaskPreparation::Ready)
            }
            Err(error) => Err(coordination_blocker(&error)),
        }
    }
}

/// A new Task inherits the schema's manual/default envelope before any Human
/// has authorized TaskCoordination. Pass `None` for that placeholder so the
/// normal authorization path derives `allowed_review_roles` from the exact
/// current Task Team. Once a Human has configured coordination, preserve the
/// configured envelope values through the parsed value.
pub(super) fn batch_baseline_is_unconfigured_default(
    baseline: &super::types::BatchCoordinatorBaseline,
) -> bool {
    baseline.unconfigured_manual_default
        && semantic_unconfigured_manual_default(
            baseline.mode.as_deref(),
            baseline.requested_by.as_deref(),
            baseline.revoked_at.as_deref(),
            baseline.envelope_json.as_deref(),
        )
}

fn batch_envelope_input(
    baseline: &super::types::BatchCoordinatorBaseline,
) -> Result<Option<Value>> {
    if batch_baseline_is_unconfigured_default(baseline) {
        return Ok(None);
    }
    let envelope_json = baseline
        .envelope_json
        .as_deref()
        .ok_or_else(|| invalid("Project batch coordination baseline has no envelope"))?;
    let envelope: CoordinationEnvelope = super::decode(envelope_json)?;
    envelope.validate()?;
    serde_json::to_value(envelope)
        .map(Some)
        .map_err(|error| invalid(error.to_string()))
}

pub(super) fn semantic_unconfigured_manual_default(
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

#[cfg(test)]
mod envelope_tests {
    use super::*;
    use serde_json::json;

    fn baseline() -> super::super::types::BatchCoordinatorBaseline {
        super::super::types::BatchCoordinatorBaseline {
            batch_version: 1,
            task_version: 1,
            task_objective_digest: "objective".to_owned(),
            lifecycle_state: "ready".to_owned(),
            has_open_work_units: false,
            has_non_cancelled_work_units: false,
            has_approved_or_materialized_plan: false,
            authorization_version: Some(1),
            team_generation: Some(1),
            team_digest: Some("team".to_owned()),
            envelope_json: Some(
                serde_json::to_string(&CoordinationEnvelope::default())
                    .expect("default envelope serializes"),
            ),
            envelope_digest: Some("envelope".to_owned()),
            mode: Some("manual".to_owned()),
            role_id: None,
            actor_kind: None,
            actor_id: None,
            requested_by: None,
            revoked_at: None,
            team_stale: Some(false),
            unconfigured_manual_default: true,
        }
    }

    #[test]
    fn a_new_manual_default_derives_allowed_review_roles_from_the_team() {
        let envelope = batch_envelope_input(&baseline()).expect("baseline is valid");
        assert!(envelope.is_none());
    }

    #[test]
    fn an_unconfigured_default_with_team_edits_before_batch_grant_is_still_a_placeholder() {
        let mut baseline = baseline();
        baseline.authorization_version = Some(4);
        baseline.team_stale = Some(true);
        assert!(batch_baseline_is_unconfigured_default(&baseline));
        assert!(batch_envelope_input(&baseline)
            .expect("default envelope is valid")
            .is_none());
    }

    #[test]
    fn an_existing_human_envelope_is_preserved() {
        let mut baseline = baseline();
        baseline.authorization_version = Some(3);
        baseline.team_stale = Some(true);
        baseline.envelope_json =
            Some(r#"{"allowed_review_roles":["quality_reviewer"],"max_work_units":2}"#.to_owned());
        let envelope = batch_envelope_input(&baseline)
            .expect("configured envelope is valid")
            .expect("configured envelope is preserved");
        assert_eq!(
            envelope,
            json!({
                "allowed_review_roles": ["quality_reviewer"],
                "max_work_units": 2
            })
        );
    }

    #[test]
    fn a_human_auth_with_default_envelope_values_is_not_treated_as_a_placeholder() {
        let mut baseline = baseline();
        baseline.requested_by = Some("owner".to_owned());
        assert!(!batch_baseline_is_unconfigured_default(&baseline));
        let envelope = batch_envelope_input(&baseline)
            .expect("configured envelope is valid")
            .expect("Human authorization is preserved");
        assert_eq!(
            envelope,
            serde_json::to_value(CoordinationEnvelope::default()).unwrap()
        );
    }

    #[test]
    fn a_historical_default_envelope_is_recognized_without_relying_on_auth_version() {
        let mut baseline = baseline();
        baseline.authorization_version = Some(5);
        baseline.envelope_json = Some(
            r#"{"allow_capability_changes":false,"allow_external_actions":false,"allow_integration":true,"allow_membership_changes":false,"allow_merge":false,"allow_new_task_roles":false,"allow_repository_write":true,"allowed_review_roles":["reviewer"],"max_work_units":4,"require_independent_review":true,"schema_version":1}"#.to_owned(),
        );
        assert!(batch_baseline_is_unconfigured_default(&baseline));
        assert!(batch_envelope_input(&baseline)
            .expect("legacy default envelope is valid")
            .is_none());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskPreparation {
    Ready,
    Completed,
}
