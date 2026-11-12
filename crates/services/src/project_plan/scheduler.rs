use std::{collections::BTreeSet, path::Path, sync::Arc};

use db::{
    ActorRef, ProjectPlanBatchRequestBinding, ProjectPlanBatchRequestOrigin, TaskCoordinationMode,
    TaskCoordinationRepo, TaskLifecycleRepo, TaskLifecycleState, TaskRepo,
};
use serde_json::{json, Value};
use sqlx::{sqlite::SqliteRow, Row, Sqlite, Transaction};

use super::{
    authorize_project_admin_in_tx, decode, digest, json_text,
    types::{BatchCoordinatorBaseline, BatchCoordinatorSnapshot, BatchProjection},
    ProjectPlanBatch, ProjectPlanDocument, ProjectPlanService,
};
use crate::{
    task_coordination::{CoordinationBrief, TaskCoordinationService, TurnRequestInput},
    Result, ServiceError,
};

const CLAIM_TTL_MINUTES: i64 = 5;
const MAX_HANDOFF_CONTEXT_BYTES: usize = 60 * 1024;

#[derive(Debug)]
pub(super) struct BatchTaskRow {
    pub(super) task_id: String,
    pub(super) state: String,
    pub(super) coordination_request_id: Option<String>,
    pub(super) blocker: Option<String>,
    pub(super) coordination_snapshot_digest: Option<String>,
}

impl ProjectPlanService {
    pub(super) async fn batch_task_objective_digest(&self, task_id: &str) -> Result<String> {
        let row = sqlx::query("SELECT project_id,title,description,repo_id,parent_task_id FROM task WHERE id=? AND deleted_at IS NULL")
            .bind(task_id).fetch_optional(self.db.pool()).await?
            .ok_or_else(|| ServiceError::not_found("task", task_id.to_owned()))?;
        task_objective_digest_from_row(&row)
    }

    pub(super) async fn read_batch_task_authority_baseline(
        &self,
        batch_id: &str,
        task_id: &str,
    ) -> Result<BatchCoordinatorBaseline> {
        let value:String=sqlx::query_scalar("SELECT authorization_baseline_json FROM project_plan_batch_task WHERE batch_id=? AND task_id=?")
            .bind(batch_id).bind(task_id).fetch_optional(self.db.pool()).await?
            .ok_or_else(||ServiceError::not_found("Project batch Task",task_id.to_owned()))?;
        decode(&value)
    }

    pub(super) async fn pin_batch_coordination_snapshot(
        &self,
        batch_id: &str,
        task_id: &str,
        user_id: &str,
        expected_task_version: i64,
        expected_authorization_version: i64,
        expected_team_generation: i64,
    ) -> Result<BatchCoordinatorSnapshot> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        let row_state = sqlx::query(
            "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
        )
        .bind(batch_id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ServiceError::not_found("Project batch Task", task_id.to_owned()))?;
        let row_state_value: String = row_state.try_get("state")?;
        let row_blocker: Option<String> = row_state.try_get("blocker")?;
        let initial_preparation =
            matches!(batch.state.as_str(), "paused" | "running") && !batch.preparation_complete;
        let lifecycle_reprepare = batch.state == "running"
            && batch.preparation_complete
            && row_state_value == "blocked"
            && row_blocker.as_deref() == Some("lifecycle:blocked");
        if batch.authorized_by != user_id || !(initial_preparation || lifecycle_reprepare) {
            return Err(db::DbError::VersionConflict.into());
        }
        authorize_project_admin_in_tx(&mut tx, &batch.project_id, user_id).await?;
        let actual_graph_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&batch.project_id)
                .fetch_one(&mut *tx)
                .await?;
        if actual_graph_version != batch.graph_version {
            return Err(db::DbError::VersionConflict.into());
        }
        let baseline_json:String=sqlx::query_scalar("SELECT authorization_baseline_json FROM project_plan_batch_task WHERE batch_id=? AND task_id=?")
            .bind(batch_id).bind(task_id).fetch_optional(&mut *tx).await?
            .ok_or_else(||ServiceError::not_found("Project batch Task",task_id.to_owned()))?;
        let baseline: BatchCoordinatorBaseline = decode(&baseline_json)?;
        let grant_row = sqlx::query(
            "SELECT project_id,authorized_by,expected_batch_version,expected_graph_version,
                    task_version,task_objective_digest,task_title,task_description,task_repo_id,
                    task_parent_task_id,team_generation,team_digest,envelope_digest,
                    unconfigured_default_refresh,mode,role_id,
                    actor_kind,actor_id
             FROM project_plan_batch_coordination_authorization
             WHERE batch_id=? AND task_id=? AND authorization_version=?",
        )
        .bind(batch_id)
        .bind(task_id)
        .bind(expected_authorization_version)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(db::DbError::ProjectPlanBatchInactive)?;
        let baseline_auth = baseline.authorization_version.ok_or_else(|| {
            ServiceError::Conflict(
                "Task coordination was not configured when the batch was authorized".into(),
            )
        })?;
        let task_version_matches = if lifecycle_reprepare {
            baseline.lifecycle_state == "blocked"
                && baseline.task_version.checked_add(1) == Some(expected_task_version)
        } else {
            baseline.task_version == expected_task_version
        };
        let unconfigured_default =
            super::batches::batch_baseline_is_unconfigured_default(&baseline);
        let allow_placeholder_refresh: bool = grant_row.try_get("unconfigured_default_refresh")?;
        if !task_version_matches
            || baseline.team_generation != Some(expected_team_generation)
            || expected_authorization_version != baseline_auth + 1
            || allow_placeholder_refresh != unconfigured_default
            || (baseline.team_stale != Some(false) && !allow_placeholder_refresh)
            || !(matches!(
                baseline.lifecycle_state.as_str(),
                "backlog" | "ready" | "blocked"
            ))
            || baseline.has_open_work_units
            || baseline.has_non_cancelled_work_units
            || baseline.has_approved_or_materialized_plan
            || baseline.team_digest.is_none()
            || baseline.envelope_digest.is_none()
            || baseline.role_id.is_none()
            || baseline.actor_kind.as_deref() != Some("agent")
            || baseline.actor_id.is_none()
        {
            return Err(db::DbError::VersionConflict.into());
        }
        let row=sqlx::query("SELECT t.id,t.project_id,t.title,t.description,t.repo_id,t.parent_task_id,t.version AS task_version,COALESCE(l.state,'backlog') AS lifecycle_state,c.authorization_version,c.team_generation,c.team_digest,c.envelope_digest,c.mode,c.coordinator_task_role_id,c.coordinator_actor_kind,c.coordinator_actor_id,c.requested_by,c.revoked_at,c.team_stale,EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status='open') AS has_open_work_units,EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status!='cancelled') AS has_non_cancelled_work_units,EXISTS(SELECT 1 FROM task_coordination_plan p WHERE p.task_id=t.id AND p.state IN ('approved','materialized')) AS has_approved_or_materialized_plan,EXISTS(SELECT 1 FROM project_plan_import_task i WHERE i.task_id=t.id AND NOT EXISTS(SELECT 1 FROM project_plan_proposal_task pt JOIN project_plan_proposal p ON p.id=pt.proposal_id WHERE pt.project_id=i.project_id AND pt.task_id=i.task_id AND pt.import_key=i.import_key AND p.state='applied' AND p.applied_version IS NOT NULL AND i.import_key='proposal:'||p.id||':node:'||pt.node_key)) AS plan_apply_incomplete FROM task t LEFT JOIN task_lifecycle l ON l.task_id=t.id JOIN task_coordination c ON c.task_id=t.id WHERE t.id=? AND t.deleted_at IS NULL")
            .bind(task_id).fetch_optional(&mut *tx).await?
            .ok_or_else(||ServiceError::not_found("Task coordination",task_id.to_owned()))?;
        let task_version: i64 = row.try_get("task_version")?;
        let task_title: String = row.try_get("title")?;
        let task_description: Option<String> = row.try_get("description")?;
        let task_repo_id: Option<String> = row.try_get("repo_id")?;
        let task_parent_task_id: Option<String> = row.try_get("parent_task_id")?;
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let team_generation: i64 = row.try_get("team_generation")?;
        let team_digest: String = row.try_get("team_digest")?;
        let envelope_digest: String = row.try_get("envelope_digest")?;
        let mode: String = row.try_get("mode")?;
        let role_id: String = row.try_get("coordinator_task_role_id")?;
        let actor_kind: String = row.try_get("coordinator_actor_kind")?;
        let actor_id: String = row.try_get("coordinator_actor_id")?;
        let requested_by: String = row.try_get("requested_by")?;
        let revoked_at: Option<String> = row.try_get("revoked_at")?;
        let team_stale: i64 = row.try_get("team_stale")?;
        let lifecycle_state: String = row.try_get("lifecycle_state")?;
        let has_open_work_units: bool = row.try_get("has_open_work_units")?;
        let has_non_cancelled_work_units: bool = row.try_get("has_non_cancelled_work_units")?;
        let has_approved_or_materialized_plan: bool =
            row.try_get("has_approved_or_materialized_plan")?;
        let plan_apply_incomplete: bool = row.try_get("plan_apply_incomplete")?;
        if plan_apply_incomplete {
            return Err(db::DbError::ProjectPlanApplyIncomplete.into());
        }
        let current_objective_digest = task_objective_digest_from_row(&row)?;
        let grant_batch_version: i64 = grant_row.try_get("expected_batch_version")?;
        let valid_grant_batch_version = if lifecycle_reprepare {
            grant_batch_version == baseline.batch_version || grant_batch_version == batch.version
        } else {
            grant_batch_version == baseline.batch_version
        };
        if grant_row.try_get::<String, _>("project_id")? != batch.project_id
            || grant_row.try_get::<String, _>("authorized_by")? != user_id
            || grant_row.try_get::<i64, _>("expected_graph_version")? != batch.graph_version
            || !valid_grant_batch_version
            || grant_row.try_get::<i64, _>("task_version")? != task_version
            || grant_row.try_get::<String, _>("task_objective_digest")?
                != baseline.task_objective_digest
            || grant_row.try_get::<String, _>("task_title")? != task_title
            || grant_row.try_get::<Option<String>, _>("task_description")? != task_description
            || grant_row.try_get::<Option<String>, _>("task_repo_id")? != task_repo_id
            || grant_row.try_get::<Option<String>, _>("task_parent_task_id")? != task_parent_task_id
            || grant_row.try_get::<i64, _>("team_generation")? != team_generation
            || grant_row.try_get::<String, _>("team_digest")? != team_digest
            || grant_row.try_get::<String, _>("envelope_digest")? != envelope_digest
            || grant_row.try_get::<String, _>("mode")? != mode
            || grant_row.try_get::<String, _>("role_id")? != role_id
            || grant_row.try_get::<String, _>("actor_kind")? != actor_kind
            || grant_row.try_get::<String, _>("actor_id")? != actor_id
        {
            return Err(db::DbError::ProjectPlanBatchInactive.into());
        }
        if task_version != expected_task_version
            || current_objective_digest != baseline.task_objective_digest
            || !matches!(lifecycle_state.as_str(), "backlog" | "ready")
            || has_open_work_units
            || has_non_cancelled_work_units
            || has_approved_or_materialized_plan
            || authorization_version != expected_authorization_version
            || team_generation != expected_team_generation
            || (!allow_placeholder_refresh
                && team_digest != baseline.team_digest.as_deref().unwrap_or_default())
            || (!allow_placeholder_refresh
                && envelope_digest != baseline.envelope_digest.as_deref().unwrap_or_default())
            || mode != "prepare_and_execute"
            || role_id != baseline.role_id.as_deref().unwrap_or_default()
            || actor_kind != "agent"
            || actor_kind != baseline.actor_kind.as_deref().unwrap_or_default()
            || actor_id != baseline.actor_id.as_deref().unwrap_or_default()
            || requested_by != user_id
            || revoked_at.is_some()
            || team_stale != 0
        {
            return Err(db::DbError::VersionConflict.into());
        }
        let snapshot = BatchCoordinatorSnapshot {
            task_version,
            task_title,
            task_description,
            task_repo_id,
            task_parent_task_id,
            authorization_version,
            team_generation,
            team_digest,
            envelope_digest,
            role_id,
            actor_kind,
            actor_id,
            requested_by,
        };
        let encoded = json_text(&snapshot)?;
        let snapshot_digest = digest(&snapshot)?;
        let result=sqlx::query("UPDATE project_plan_batch_task SET coordination_snapshot_json=COALESCE(coordination_snapshot_json,?),coordination_snapshot_digest=COALESCE(coordination_snapshot_digest,?) WHERE batch_id=? AND task_id=? AND (coordination_snapshot_digest IS NULL OR coordination_snapshot_digest=?)")
            .bind(&encoded).bind(&snapshot_digest).bind(batch_id).bind(task_id).bind(&snapshot_digest).execute(&mut *tx).await?;
        if result.rows_affected() != 1 {
            return Err(db::DbError::VersionConflict.into());
        }
        tx.commit().await?;
        Ok(snapshot)
    }
    /// Recovery and admission tick for authorized Project batches. Reservations
    /// are committed before a coordination request; request IDs are stable per
    /// batch+Task so a crash after request creation replays the exact request.
    pub async fn check_once(&self) -> Result<u64> {
        let preparing = sqlx::query_scalar::<_, String>(
            "SELECT id FROM project_plan_batch WHERE state='paused' AND preparation_complete=0 ORDER BY created_at,id",
        ).fetch_all(self.db.pool()).await?;
        for batch_id in preparing {
            self.prepare_batch(&batch_id).await?;
        }

        let batch_ids = sqlx::query_scalar::<_, String>(
            "SELECT id FROM project_plan_batch WHERE state='running' ORDER BY created_at,id",
        )
        .fetch_all(self.db.pool())
        .await?;
        let mut requested = 0_u64;
        for batch_id in batch_ids {
            requested += self.drive_batch_once(&batch_id).await?;
        }
        Ok(requested)
    }

    /// Verify that a repository successor Workspace includes every exact
    /// integrated predecessor commit pinned by this batch's handoff.
    pub async fn verify_handoff_workspace_base(
        &self,
        task_id: &str,
        work_unit_id: &str,
        workspace: Option<&db::Workspace>,
    ) -> Result<()> {
        let requires_integration: bool = sqlx::query_scalar(
            "SELECT requires_integration FROM work_unit WHERE id=? AND task_id=?",
        )
        .bind(work_unit_id)
        .bind(task_id)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| ServiceError::not_found("WorkUnit", work_unit_id.to_owned()))?;
        if !requires_integration {
            return Ok(());
        }
        let handoffs = sqlx::query(
            "SELECT b.id AS batch_id,h.context_json,h.digest
             FROM project_plan_handoff h
             JOIN project_plan_batch b ON b.id=h.batch_id
             JOIN project_plan_batch_task bt ON bt.batch_id=b.id AND bt.task_id=h.task_id
             WHERE h.task_id=? AND b.state IN ('running','paused')
               AND bt.state IN ('claimed','started','blocked')
             ORDER BY b.created_at,b.id,h.predecessor_task_id",
        )
        .bind(task_id)
        .fetch_all(self.db.pool())
        .await?;
        let Some(workspace) = workspace else {
            for row in &handoffs {
                let batch_id: String = row.try_get("batch_id")?;
                self.pause_for_handoff_blocker(
                    &batch_id,
                    task_id,
                    "handoff:repository_workspace_missing",
                )
                .await?;
            }
            if !handoffs.is_empty() {
                return Err(db::DbError::ProjectDependencyUnsatisfied.into());
            }
            return Err(ServiceError::invalid_operation(
                "repository WorkUnit has no exact Workspace base",
            ));
        };
        let exact = db::WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation("repository WorkUnit has no exact Workspace scope")
            })?;
        let scope = db::WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &exact.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace scope", exact.id.clone()))?;
        if exact.id != workspace.id
            || exact.repo_id != workspace.repo_id
            || exact.worktree_path != workspace.worktree_path
            || exact.before_sha != workspace.before_sha
            || scope.task_id != task_id
            || scope.kind != db::WorkspaceScopeKind::WorkUnit
            || scope.work_unit_id.as_deref() != Some(work_unit_id)
            || workspace.status != db::WorkspaceStatus::Ready
        {
            for row in &handoffs {
                let batch_id: String = row.try_get("batch_id")?;
                self.pause_for_handoff_blocker(
                    &batch_id,
                    task_id,
                    "handoff:workspace_identity_changed",
                )
                .await?;
            }
            if !handoffs.is_empty() {
                return Err(db::DbError::ProjectDependencyUnsatisfied.into());
            }
            return Err(ServiceError::invalid_operation(
                "Workspace is not the exact ready Workspace for this WorkUnit",
            ));
        }
        for row in handoffs {
            let batch_id: String = row.try_get("batch_id")?;
            let context_json: String = row.try_get("context_json")?;
            let stored_digest: String = row.try_get("digest")?;
            let context: Value = decode(&context_json)?;
            if digest(&context)? != stored_digest {
                self.pause_for_handoff_blocker(
                    &batch_id,
                    task_id,
                    "handoff:pinned_context_corrupt",
                )
                .await?;
                return Err(db::DbError::ProjectDependencyUnsatisfied.into());
            }
            let predecessors = match context.get("direct_predecessors").and_then(Value::as_array) {
                Some(values) => values.iter().collect::<Vec<_>>(),
                None if context
                    .get("work_units")
                    .and_then(Value::as_array)
                    .is_some() =>
                {
                    vec![&context]
                }
                None => {
                    self.pause_for_handoff_blocker(
                        &batch_id,
                        task_id,
                        "handoff:pinned_context_schema_invalid",
                    )
                    .await?;
                    return Err(db::DbError::ProjectDependencyUnsatisfied.into());
                }
            };
            let mut mismatch = None;
            for predecessor in predecessors {
                let Some(work_units) = predecessor.get("work_units").and_then(Value::as_array)
                else {
                    mismatch = Some("handoff:pinned_context_schema_invalid".to_owned());
                    break;
                };
                for unit in work_units {
                    let Some(requires_integration) =
                        unit.get("requires_integration").and_then(Value::as_bool)
                    else {
                        mismatch = Some("handoff:pinned_context_schema_invalid".to_owned());
                        break;
                    };
                    if !requires_integration {
                        continue;
                    }
                    let Some(integration) = unit.get("integration").filter(|v| !v.is_null()) else {
                        mismatch = Some("handoff:integration_evidence_missing".to_owned());
                        break;
                    };
                    let Some(repo_id) = integration.get("repo_id").and_then(Value::as_str) else {
                        mismatch = Some("handoff:integration_repo_missing".to_owned());
                        break;
                    };
                    if repo_id != workspace.repo_id {
                        continue;
                    }
                    let Some(commit_sha) = integration.get("commit_sha").and_then(Value::as_str)
                    else {
                        mismatch = Some("handoff:integration_commit_missing".to_owned());
                        break;
                    };
                    let Some(base_sha) = workspace.before_sha.as_deref() else {
                        mismatch = Some(format!(
                            "handoff:workspace_base_missing:{}:{commit_sha}",
                            workspace.repo_id
                        ));
                        break;
                    };
                    match git::is_ancestor(
                        Path::new(&workspace.worktree_path),
                        commit_sha,
                        base_sha,
                    )
                    .await
                    {
                        Ok(true) => (),
                        Ok(false) => {
                            mismatch = Some(format!(
                                "handoff:workspace_base_missing_commit:{}:{commit_sha}",
                                workspace.repo_id
                            ));
                            break;
                        }
                        Err(_) => {
                            mismatch = Some(format!(
                                "handoff:workspace_ancestry_unverified:{}:{commit_sha}",
                                workspace.repo_id
                            ));
                            break;
                        }
                    }
                }
                if mismatch.is_some() {
                    break;
                }
            }
            if let Some(reason) = mismatch {
                self.pause_for_handoff_blocker(&batch_id, task_id, &reason)
                    .await?;
                tracing::warn!(task_id, work_unit_id, batch_id, blocker = %reason, "project DAG handoff requires human workspace repair");
                return Err(db::DbError::ProjectDependencyUnsatisfied.into());
            }
        }
        Ok(())
    }

    /// ProviderStart repeats the workspace ancestry check after Execution
    /// preparation so direct/manual launch paths cannot bypass a pinned handoff.
    pub async fn verify_execution_handoff_workspace_base(
        &self,
        execution: &db::Execution,
    ) -> Result<()> {
        let Some(work_unit_id) = execution.work_unit_id.as_deref() else {
            return Ok(());
        };
        let Some(work_unit) = db::WorkUnitRepo::get_by_id(&*self.db, work_unit_id).await? else {
            return Err(db::DbError::ProjectDependencyUnsatisfied.into());
        };
        if !work_unit.requires_integration {
            return Ok(());
        }
        let workspace =
            db::WorkUnitWorkspaceRepo::get_by_work_unit_id(&*self.db, work_unit_id).await?;
        if workspace.as_ref().is_some_and(|workspace| {
            execution.workspace_id.as_deref() != Some(workspace.id.as_str())
        }) {
            return Err(db::DbError::ProjectDependencyUnsatisfied.into());
        }
        self.verify_handoff_workspace_base(&execution.task_id, work_unit_id, workspace.as_ref())
            .await
    }

    async fn pause_for_handoff_blocker(
        &self,
        batch_id: &str,
        task_id: &str,
        blocker: &str,
    ) -> Result<()> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        if !matches!(batch.state.as_str(), "running" | "paused") {
            tx.commit().await?;
            return Ok(());
        }
        let changed = batch.state == "running";
        if changed {
            sqlx::query("UPDATE project_plan_batch SET state='paused',version=version+1,updated_at=? WHERE id=? AND state='running'")
                .bind(db::now_rfc3339()).bind(batch_id).execute(&mut *tx).await?;
        }
        sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker=? WHERE batch_id=? AND task_id=? AND state IN ('claimed','started','blocked')")
            .bind(blocker).bind(batch_id).bind(task_id).execute(&mut *tx).await?;
        let version: i64 = sqlx::query_scalar("SELECT version FROM project_plan_batch WHERE id=?")
            .bind(batch_id)
            .fetch_one(&mut *tx)
            .await?;
        let event = if changed {
            Some(self.system_event_with_payload(&mut tx, &batch.project_id, "project.plan_batch_updated", batch_id, json!({"project_id":batch.project_id,"batch_id":batch_id,"graph_version":batch.graph_version,"version":version,"state":"paused"})).await?)
        } else {
            None
        };
        tx.commit().await?;
        if let Some(event) = event {
            self.publish(&event);
        }
        Ok(())
    }

    async fn drive_batch_once(&self, batch_id: &str) -> Result<u64> {
        let batch = self.load_batch_projection(batch_id).await?;
        if batch.state != "running" || !batch.preparation_complete {
            return Ok(0);
        }
        let actual_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&batch.project_id)
                .fetch_one(self.db.pool())
                .await?;
        if batch.graph_version != actual_version || !self.batch_authority_current(&batch).await? {
            self.pause_stale_batch(
                &batch,
                if batch.graph_version != actual_version {
                    "plan:graph_version_changed"
                } else {
                    "batch:authorization_revoked"
                },
            )
            .await?;
            return Ok(0);
        }
        self.recover_expired_claims(&batch).await?;
        self.refresh_batch_completions(&batch).await?;
        if self.complete_batch_if_done(&batch).await? {
            return Ok(0);
        }

        let before_retry = self.batch_task_rows(batch_id).await?;
        let mut reauthorized = BTreeSet::new();
        for row in before_retry {
            if row.state == "blocked" && row.blocker.as_deref() == Some("lifecycle:blocked") {
                let lifecycle =
                    db::TaskLifecycleRepo::get_task_lifecycle(&*self.db, &row.task_id).await?;
                if lifecycle.as_ref().is_some_and(|value| {
                    matches!(
                        value.state,
                        TaskLifecycleState::Backlog | TaskLifecycleState::Ready
                    )
                }) && self
                    .prepare_waiting_lifecycle_task(batch_id, &row.task_id)
                    .await?
                {
                    reauthorized.insert(row.task_id);
                }
            }
        }
        let task_rows = self.batch_task_rows(batch_id).await?;
        let mut started = 0_u64;
        for row in task_rows {
            if reauthorized.contains(&row.task_id) {
                continue;
            }
            if row.state != "pending"
                && !(row.state == "blocked"
                    && row
                        .blocker
                        .as_deref()
                        .is_some_and(|blocker| blocker.starts_with("dependency:")))
                && !(row.state == "blocked"
                    && row.blocker.as_deref() == Some("coordination:coordination.budget_exhausted"))
            {
                continue;
            }
            if self
                .is_stopped_by_coordinator_error(row.task_id.as_str())
                .await?
            {
                self.update_batch_task(
                    batch_id,
                    &row.task_id,
                    "blocked",
                    None,
                    None,
                    Some("coordination:authorization_required"),
                )
                .await?;
                continue;
            }
            match self.claim_batch_task(&batch, &row.task_id).await? {
                Some((context, claim_token)) => {
                    let coordination =
                        TaskCoordinationRepo::get_task_coordination(&*self.db, &row.task_id)
                            .await?
                            .ok_or_else(|| {
                                ServiceError::Conflict(
                                    "Task coordination authorization disappeared after reservation"
                                        .into(),
                                )
                            })?;
                    let task = TaskRepo::get_by_id(&*self.db, &row.task_id, false)
                        .await?
                        .ok_or_else(|| ServiceError::not_found("task", row.task_id.clone()))?;
                    let brief = self.coordination_brief(&batch, &task, &context).await?;
                    let idempotency_key =
                        format!("project-plan-batch:{}:turn:{}", batch.id, row.task_id);
                    let snapshot_digest = row
                        .coordination_snapshot_digest
                        .as_deref()
                        .ok_or(db::DbError::ProjectPlanBatchInactive)?;
                    let binding = ProjectPlanBatchRequestBinding {
                        project_id: batch.project_id.clone(),
                        batch_id: batch.id.clone(),
                        authorized_by: batch.authorized_by.clone(),
                        expected_batch_version: batch.version,
                        expected_graph_version: batch.graph_version,
                        expected_task_version: task.version,
                        expected_coordination_version: coordination.authorization_version,
                        coordination_snapshot_digest: snapshot_digest.to_owned(),
                        origin: ProjectPlanBatchRequestOrigin::Scheduler {
                            claim_token: claim_token.clone(),
                        },
                    };
                    let service = TaskCoordinationService::new(
                        Arc::clone(&self.db),
                        Arc::clone(&self.event_bus),
                    );
                    match service
                        .request_project_batch_turn(
                            &row.task_id,
                            &batch.authorized_by,
                            TurnRequestInput {
                                expected_task_version: task.version,
                                expected_coordination_version: coordination.authorization_version,
                                brief,
                                idempotency_key,
                            },
                            binding,
                        )
                        .await
                    {
                        Ok((_request, _created)) => {
                            started += 1;
                        }
                        Err(error) => {
                            self.release_batch_claim(
                                batch_id,
                                &row.task_id,
                                &claim_token,
                                batch.version,
                                Some(&coordination_blocker(&error)),
                            )
                            .await?
                        }
                    }
                }
                None => {}
            }
        }
        self.refresh_batch_completions(&batch).await?;
        self.complete_batch_if_done(&batch).await?;
        Ok(started)
    }

    async fn claim_batch_task(
        &self,
        batch: &BatchProjection,
        task_id: &str,
    ) -> Result<Option<(Value, String)>> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let current = batch_projection_by_id_in_tx(&mut tx, &batch.id).await?;
        if current.state != "running"
            || !current.preparation_complete
            || current.version != batch.version
            || current.graph_version != batch.graph_version
        {
            tx.commit().await?;
            return Ok(None);
        }
        authorize_project_admin_in_tx(&mut tx, &current.project_id, &current.authorized_by).await?;
        let graph_version: i64 =
            sqlx::query_scalar("SELECT version FROM project_plan WHERE project_id=?")
                .bind(&current.project_id)
                .fetch_one(&mut *tx)
                .await?;
        if graph_version != current.graph_version {
            tx.commit().await?;
            return Ok(None);
        }
        let task_row = sqlx::query(
            "SELECT state,blocker FROM project_plan_batch_task WHERE batch_id=? AND task_id=?",
        )
        .bind(&current.id)
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(task_row) = task_row else {
            tx.commit().await?;
            return Ok(None);
        };
        let task_state: String = task_row.try_get("state")?;
        let task_blocker: Option<String> = task_row.try_get("blocker")?;
        let retryable = task_state == "pending"
            || (task_state == "blocked"
                && task_blocker
                    .as_deref()
                    .is_some_and(|blocker| blocker.starts_with("dependency:")))
            || (task_state == "blocked"
                && task_blocker.as_deref() == Some("coordination:coordination.budget_exhausted"));
        if !retryable {
            tx.commit().await?;
            return Ok(None);
        }
        let conflict:bool=sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM project_plan_batch_task bt JOIN project_plan_batch other ON other.id=bt.batch_id
             WHERE bt.task_id=? AND bt.batch_id!=? AND other.state IN ('running','paused'))",
        ).bind(task_id).bind(&current.id).fetch_one(&mut *tx).await?;
        if conflict {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',blocker='batch:task_reserved' WHERE batch_id=? AND task_id=?")
            .bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let lifecycle: Option<String> =
            sqlx::query_scalar("SELECT state FROM task_lifecycle WHERE task_id=?")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?;
        let lifecycle = lifecycle.unwrap_or_else(|| "unknown".into());
        if !matches!(lifecycle.as_str(), "backlog" | "ready") {
            let blocker = if lifecycle == "done" {
                "delivery:integration_or_acceptance_required".to_owned()
            } else {
                format!("lifecycle:{lifecycle}")
            };
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',blocker=? WHERE batch_id=? AND task_id=?")
                .bind(blocker).bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let plan_apply_incomplete:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_plan_import_task i WHERE i.task_id=? AND NOT EXISTS(SELECT 1 FROM project_plan_proposal_task pt JOIN project_plan_proposal p ON p.id=pt.proposal_id WHERE pt.project_id=i.project_id AND pt.task_id=i.task_id AND pt.import_key=i.import_key AND p.state='applied' AND p.applied_version IS NOT NULL AND i.import_key='proposal:'||p.id||':node:'||pt.node_key))")
            .bind(task_id).fetch_one(&mut *tx).await?;
        if plan_apply_incomplete {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker='plan:apply_incomplete' WHERE batch_id=? AND task_id=?")
                .bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let dispatch: Option<String> =
            sqlx::query_scalar("SELECT state FROM task_dispatch WHERE task_id=?")
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?;
        if matches!(dispatch.as_deref(), Some("paused" | "cancelled")) {
            let blocker = format!("dispatch:{}", dispatch.as_deref().unwrap_or("unknown"));
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',blocker=? WHERE batch_id=? AND task_id=?")
                .bind(blocker).bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let unsatisfied=sqlx::query_scalar::<_,String>("SELECT depends_on_id FROM project_task_dependency_readiness WHERE task_id=? AND satisfied=0 ORDER BY depends_on_id")
            .bind(task_id).fetch_all(&mut *tx).await?;
        if let Some(prerequisite) = unsatisfied.first() {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',blocker=? WHERE batch_id=? AND task_id=?")
                .bind(format!("dependency:{prerequisite}")).bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let coordination_current = batch_coordination_snapshot_current_in_tx(
            &mut tx,
            &current.id,
            task_id,
            &current.authorized_by,
        )
        .await?;
        if !coordination_current {
            sqlx::query("UPDATE project_plan_batch_task SET state='blocked',blocker='coordination:authorization_changed' WHERE batch_id=? AND task_id=?")
                .bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let budget_available: bool = sqlx::query_scalar(
            "SELECT turn_count < max_turns FROM task_coordination WHERE task_id=?",
        )
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or(false);
        if task_state == "blocked"
            && task_blocker.as_deref() == Some("coordination:coordination.budget_exhausted")
            && !budget_available
        {
            tx.commit().await?;
            return Ok(None);
        }
        let (occupied, task_occupied) =
            project_capacity_in_tx(&mut tx, &current.project_id, task_id).await?;
        if occupied >= current.max_parallel_tasks && !task_occupied {
            sqlx::query("UPDATE project_plan_batch_task SET state='pending',blocker='capacity:project_parallel_limit' WHERE batch_id=? AND task_id=?")
                .bind(&current.id).bind(task_id).execute(&mut *tx).await?;
            tx.commit().await?;
            return Ok(None);
        }
        let context = match build_and_pin_handoffs_in_tx(&mut tx, &current, task_id).await {
            Ok(context) => context,
            Err(ServiceError::Conflict(message)) => {
                let blocker = handoff_blocker_code(&message);
                sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker=? WHERE batch_id=? AND task_id=?")
                    .bind(&blocker).bind(&current.id).bind(task_id).execute(&mut *tx).await?;
                tx.commit().await?;
                tracing::warn!(task_id,batch_id=%current.id,blocker=%blocker,detail=%message,"project DAG successor is blocked pending exact prerequisite handoff evidence");
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let expires =
            (chrono::Utc::now() + chrono::Duration::minutes(CLAIM_TTL_MINUTES)).to_rfc3339();
        let claim_token = db::new_uuid_v4();
        let update=sqlx::query("UPDATE project_plan_batch_task SET state='claimed',claim_expires_at=?,claim_token=?,blocker=NULL WHERE batch_id=? AND task_id=? AND (state='pending' OR (state='blocked' AND blocker LIKE 'dependency:%') OR (state='blocked' AND blocker='coordination:coordination.budget_exhausted' AND EXISTS(SELECT 1 FROM task_coordination c WHERE c.task_id=? AND c.turn_count<c.max_turns)))")
            .bind(expires).bind(&claim_token).bind(&current.id).bind(task_id).bind(task_id).execute(&mut *tx).await?;
        if update.rows_affected() != 1 {
            tx.commit().await?;
            return Ok(None);
        }
        tx.commit().await?;
        Ok(Some((context, claim_token)))
    }

    async fn release_batch_claim(
        &self,
        batch_id: &str,
        task_id: &str,
        claim_token: &str,
        expected_batch_version: i64,
        blocker: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let update = sqlx::query(
            "UPDATE project_plan_batch_task
             SET state='blocked',claim_expires_at=NULL,claim_token=NULL,blocker=?
             WHERE batch_id=? AND task_id=? AND state='claimed' AND claim_token=?
              AND EXISTS(SELECT 1 FROM project_plan_batch b
                         WHERE b.id=? AND b.version=? AND b.state='running'
                          AND b.preparation_complete=1)",
        )
        .bind(blocker)
        .bind(batch_id)
        .bind(task_id)
        .bind(claim_token)
        .bind(batch_id)
        .bind(expected_batch_version)
        .execute(&mut *tx)
        .await?;
        if update.rows_affected() > 0 {
            tx.commit().await?;
        } else {
            tx.rollback().await?;
        }
        Ok(())
    }

    async fn recover_expired_claims(&self, batch: &BatchProjection) -> Result<()> {
        let now = db::now_rfc3339();
        let expired=sqlx::query("SELECT task_id FROM project_plan_batch_task WHERE batch_id=? AND state='claimed' AND claim_expires_at IS NOT NULL AND claim_expires_at<? ORDER BY task_id")
            .bind(&batch.id).bind(&now).fetch_all(self.db.pool()).await?;
        for row in expired {
            let task_id: String = row.try_get("task_id")?;
            self.update_batch_task(&batch.id, &task_id, "pending", None, None, None)
                .await?;
        }
        Ok(())
    }

    async fn refresh_batch_completions(&self, batch: &BatchProjection) -> Result<()> {
        let task_rows = self.batch_task_rows(&batch.id).await?;
        for row in task_rows {
            if row.state == "completed" {
                continue;
            }
            let lifecycle = TaskLifecycleRepo::get_task_lifecycle(&*self.db, &row.task_id).await?;
            if lifecycle
                .as_ref()
                .is_some_and(|lifecycle| lifecycle.state == TaskLifecycleState::Done)
            {
                if self.task_delivery_complete(&row.task_id).await? {
                    self.update_batch_task(
                        &batch.id,
                        &row.task_id,
                        "completed",
                        row.coordination_request_id.as_deref(),
                        None,
                        None,
                    )
                    .await?;
                } else {
                    self.update_batch_task(
                        &batch.id,
                        &row.task_id,
                        "blocked",
                        row.coordination_request_id.as_deref(),
                        None,
                        Some("delivery:integration_or_acceptance_required"),
                    )
                    .await?;
                }
            }
        }
        Ok(())
    }

    async fn complete_batch_if_done(&self, batch: &BatchProjection) -> Result<bool> {
        let unfinished:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_plan_batch_task WHERE batch_id=? AND state!='completed')")
            .bind(&batch.id).fetch_one(self.db.pool()).await?;
        if unfinished {
            return Ok(false);
        }
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let current = batch_projection_by_id_in_tx(&mut tx, &batch.id).await?;
        if current.state != "running" {
            tx.commit().await?;
            return Ok(false);
        }
        let now = db::now_rfc3339();
        sqlx::query("UPDATE project_plan_batch SET state='completed',version=version+1,updated_at=? WHERE id=? AND state='running'")
            .bind(&now).bind(&batch.id).execute(&mut *tx).await?;
        let version: i64 = sqlx::query_scalar("SELECT version FROM project_plan_batch WHERE id=?")
            .bind(&batch.id)
            .fetch_one(&mut *tx)
            .await?;
        let event=self.system_event_with_payload(&mut tx,&current.project_id,"project.plan_batch_updated",&batch.id,json!({"project_id":current.project_id,"batch_id":batch.id,"graph_version":batch.graph_version,"version":version,"state":"completed"})).await?;
        tx.commit().await?;
        self.publish(&event);
        Ok(true)
    }

    async fn pause_stale_batch(&self, batch: &BatchProjection, reason: &str) -> Result<()> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let current = batch_projection_by_id_in_tx(&mut tx, &batch.id).await?;
        if current.state != "running" {
            tx.commit().await?;
            return Ok(());
        }
        sqlx::query("UPDATE project_plan_batch SET state='paused',version=version+1,updated_at=? WHERE id=? AND state='running'")
            .bind(db::now_rfc3339()).bind(&batch.id).execute(&mut *tx).await?;
        sqlx::query("UPDATE project_plan_batch_task SET state='blocked',claim_expires_at=NULL,blocker=? WHERE batch_id=? AND state IN ('pending','claimed')")
            .bind(reason).bind(&batch.id).execute(&mut *tx).await?;
        let version: i64 = sqlx::query_scalar("SELECT version FROM project_plan_batch WHERE id=?")
            .bind(&batch.id)
            .fetch_one(&mut *tx)
            .await?;
        let event=self.system_event_with_payload(&mut tx,&batch.project_id,"project.plan_batch_updated",&batch.id,json!({"project_id":batch.project_id,"batch_id":batch.id,"graph_version":batch.graph_version,"version":version,"state":"paused"})).await?;
        tx.commit().await?;
        self.publish(&event);
        Ok(())
    }

    pub(super) async fn task_delivery_complete(&self, task_id: &str) -> Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM task t JOIN task_lifecycle l ON l.task_id=t.id
             WHERE t.id=? AND t.deleted_at IS NULL AND l.state='done'
              AND NOT EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status NOT IN ('completed','cancelled'))
              AND NOT EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.requires_integration=1 AND w.status!='cancelled'
               AND (w.status!='completed' OR NOT EXISTS(
                SELECT 1 FROM work_unit_integration i JOIN execution e ON e.id=i.execution_id AND e.task_id=i.task_id
                WHERE i.work_unit_id=w.id AND i.task_id=w.task_id AND i.outcome='success' AND e.status='completed'
                 AND e.work_unit_version=w.version-1 AND e.after_sha=i.source_sha AND i.target_after_sha IS NOT NULL))))",
        ).bind(task_id).fetch_one(self.db.pool()).await?)
    }

    async fn coordination_brief(
        &self,
        batch: &BatchProjection,
        task: &db::Task,
        handoff_context: &Value,
    ) -> Result<CoordinationBrief> {
        let document_json: String =
            sqlx::query_scalar("SELECT document_json FROM project_plan WHERE project_id=?")
                .bind(&batch.project_id)
                .fetch_one(self.db.pool())
                .await?;
        let document: ProjectPlanDocument = decode(&document_json)?;
        let node = document
            .nodes
            .iter()
            .find(|node| node.task_id.as_deref() == Some(task.id.as_str()));
        let objective = node
            .map(|node| format!("Deliver project DAG node {}: {}", node.key, node.title))
            .unwrap_or_else(|| format!("Coordinate delivery of Task {}: {}", task.id, task.title));
        let criteria = node
            .map(|node| node.acceptance_criteria.clone())
            .unwrap_or_default();
        let context = json!({
            "project_id":batch.project_id,"batch_id":batch.id,"graph_version":batch.graph_version,
            "task_id":task.id,"node_key":node.map(|node|node.key.as_str()),
            "description":node.and_then(|node|node.description.as_deref()),
            "phase":node.and_then(|node|node.phase.as_deref()),
            "source_ids":node.map(|node|node.source_ids.as_slice()).unwrap_or_default(),
            "pinned_predecessors":handoff_context
        });
        Ok(CoordinationBrief{
            objective,
            constraints:Some("Respect the exact TaskRole and Actor authorization, preserve the WorkUnit dependency DAG, and do not claim that Task completion alone satisfies a Project prerequisite. Required repository WorkUnits must complete and integrate through the existing integration flow. Do not merge or bypass review Gates.".into()),
            acceptance_criteria:criteria,
            context:Some(json_text(&context)?),
        })
    }

    async fn is_stopped_by_coordinator_error(&self, task_id: &str) -> Result<bool> {
        let row = TaskCoordinationRepo::get_task_coordination(&*self.db, task_id).await?;
        Ok(!row.is_some_and(|coordination| {
            coordination.mode == TaskCoordinationMode::PrepareAndExecute
                && coordination.revoked_at.is_none()
                && !coordination.team_stale
                && coordination.coordinator_task_role_id.is_some()
                && matches!(coordination.coordinator_actor, Some(ActorRef::Agent(_)))
        }))
    }

    async fn batch_authority_current(&self, batch: &BatchProjection) -> Result<bool> {
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        let current =
            project_admin_current_in_tx(&mut tx, &batch.project_id, &batch.authorized_by).await?;
        tx.commit().await?;
        Ok(current)
    }

    pub(super) async fn update_batch_task(
        &self,
        batch_id: &str,
        task_id: &str,
        state: &str,
        request_id: Option<&str>,
        expires: Option<&str>,
        blocker: Option<&str>,
    ) -> Result<()> {
        sqlx::query("UPDATE project_plan_batch_task SET state=?,coordination_request_id=COALESCE(?,coordination_request_id),claim_expires_at=?,claim_token=NULL,blocker=? WHERE batch_id=? AND task_id=? AND EXISTS(SELECT 1 FROM project_plan_batch b WHERE b.id=? AND b.state IN ('running','paused'))")
            .bind(state).bind(request_id).bind(expires).bind(blocker).bind(batch_id).bind(task_id).bind(batch_id).execute(self.db.pool()).await?;
        Ok(())
    }

    pub(super) async fn batch_task_rows(&self, batch_id: &str) -> Result<Vec<BatchTaskRow>> {
        Ok(sqlx::query("SELECT task_id,state,coordination_request_id,blocker,coordination_snapshot_digest FROM project_plan_batch_task WHERE batch_id=? ORDER BY task_id")
            .bind(batch_id).fetch_all(self.db.pool()).await?.into_iter().map(|row|Ok(BatchTaskRow{
                task_id:row.try_get("task_id")?,state:row.try_get("state")?,
                coordination_request_id:row.try_get("coordination_request_id")?,blocker:row.try_get("blocker")?,
                coordination_snapshot_digest:row.try_get("coordination_snapshot_digest")?,
            })).collect::<std::result::Result<Vec<_>,sqlx::Error>>()?)
    }

    pub(super) async fn load_batch(&self, batch_id: &str) -> Result<ProjectPlanBatch> {
        let mut tx = self.db.pool().begin().await?;
        let batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        let task_ids = sqlx::query_scalar::<_, String>(
            "SELECT task_id FROM project_plan_batch_task WHERE batch_id=? ORDER BY task_id",
        )
        .bind(batch_id)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(ProjectPlanBatch {
            id: batch.id,
            project_id: batch.project_id,
            graph_version: batch.graph_version,
            version: batch.version,
            state: batch.state,
            task_ids,
            max_parallel_tasks: batch.max_parallel_tasks,
            authorized_by: batch.authorized_by,
            created_at: batch.created_at,
            updated_at: batch.updated_at,
        })
    }

    pub(super) async fn load_batch_projection(&self, batch_id: &str) -> Result<BatchProjection> {
        let mut tx = self.db.pool().begin().await?;
        let batch = batch_projection_by_id_in_tx(&mut tx, batch_id).await?;
        tx.commit().await?;
        Ok(batch)
    }
}

pub(super) async fn insert_batch_task_with_authority_baseline_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    batch_id: &str,
    task_id: &str,
) -> Result<()> {
    let batch_version: i64 =
        sqlx::query_scalar("SELECT version FROM project_plan_batch WHERE id=?")
            .bind(batch_id)
            .fetch_optional(&mut **tx)
            .await?
            .ok_or_else(|| ServiceError::not_found("Project batch", batch_id.to_owned()))?;
    let row=sqlx::query("SELECT t.version AS task_version,COALESCE(l.state,'backlog') AS lifecycle_state,EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status='open') AS has_open_work_units,EXISTS(SELECT 1 FROM work_unit w WHERE w.task_id=t.id AND w.status!='cancelled') AS has_non_cancelled_work_units,EXISTS(SELECT 1 FROM task_coordination_plan p WHERE p.task_id=t.id AND p.state IN ('approved','materialized')) AS has_approved_or_materialized_plan,c.authorization_version,c.team_generation,c.team_digest,c.envelope_json,c.envelope_digest,c.mode,c.coordinator_task_role_id,c.coordinator_actor_kind,c.coordinator_actor_id,c.requested_by,c.revoked_at,c.team_stale,t.project_id,t.title,t.description,t.repo_id,t.parent_task_id FROM task t LEFT JOIN task_lifecycle l ON l.task_id=t.id LEFT JOIN task_coordination c ON c.task_id=t.id WHERE t.id=? AND t.deleted_at IS NULL")
        .bind(task_id).fetch_optional(&mut **tx).await?.ok_or_else(||ServiceError::not_found("task",task_id.to_owned()))?;
    let mut baseline = BatchCoordinatorBaseline {
        batch_version,
        task_version: row.try_get("task_version")?,
        task_objective_digest: task_objective_digest_from_row(&row)?,
        lifecycle_state: row.try_get("lifecycle_state")?,
        has_open_work_units: row.try_get("has_open_work_units")?,
        has_non_cancelled_work_units: row.try_get("has_non_cancelled_work_units")?,
        has_approved_or_materialized_plan: row.try_get("has_approved_or_materialized_plan")?,
        authorization_version: row.try_get("authorization_version")?,
        team_generation: row.try_get("team_generation")?,
        team_digest: row.try_get("team_digest")?,
        envelope_json: row.try_get("envelope_json")?,
        envelope_digest: row.try_get("envelope_digest")?,
        mode: row.try_get("mode")?,
        role_id: row.try_get("coordinator_task_role_id")?,
        actor_kind: row.try_get("coordinator_actor_kind")?,
        actor_id: row.try_get("coordinator_actor_id")?,
        requested_by: row.try_get("requested_by")?,
        revoked_at: row.try_get("revoked_at")?,
        team_stale: row
            .try_get::<Option<i64>, _>("team_stale")?
            .map(|value| value != 0),
        unconfigured_manual_default: false,
    };
    baseline.unconfigured_manual_default = super::batches::semantic_unconfigured_manual_default(
        baseline.mode.as_deref(),
        baseline.requested_by.as_deref(),
        baseline.revoked_at.as_deref(),
        baseline.envelope_json.as_deref(),
    );
    sqlx::query("INSERT INTO project_plan_batch_task(batch_id,task_id,state,authorization_baseline_json) VALUES(?,?,'pending',?)")
        .bind(batch_id).bind(task_id).bind(json_text(&baseline)?).execute(&mut **tx).await?;
    Ok(())
}

fn task_objective_digest_from_row(row: &SqliteRow) -> Result<String> {
    Ok(digest(&(
        row.try_get::<String, _>("project_id")?,
        row.try_get::<String, _>("title")?,
        row.try_get::<Option<String>, _>("description")?,
        row.try_get::<Option<String>, _>("repo_id")?,
        row.try_get::<Option<String>, _>("parent_task_id")?,
    ))?)
}

pub(super) async fn batch_coordination_snapshot_current_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    batch_id: &str,
    task_id: &str,
    authorized_by: &str,
) -> Result<bool> {
    let row=sqlx::query("SELECT coordination_snapshot_json,coordination_snapshot_digest FROM project_plan_batch_task WHERE batch_id=? AND task_id=?")
        .bind(batch_id).bind(task_id).fetch_optional(&mut **tx).await?;
    let Some(row) = row else { return Ok(false) };
    let Some(encoded) = row.try_get::<Option<String>, _>("coordination_snapshot_json")? else {
        return Ok(false);
    };
    let Some(stored_digest) = row.try_get::<Option<String>, _>("coordination_snapshot_digest")?
    else {
        return Ok(false);
    };
    let snapshot: BatchCoordinatorSnapshot = decode(&encoded)?;
    if digest(&snapshot)? != stored_digest
        || snapshot.requested_by != authorized_by
        || snapshot.actor_kind != "agent"
    {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_coordination c JOIN task t ON t.id=c.task_id WHERE c.task_id=? AND t.version=? AND t.project_id=(SELECT project_id FROM project_plan_batch WHERE id=?) AND t.title=? AND t.description IS ? AND t.repo_id IS ? AND t.parent_task_id IS ? AND c.mode='prepare_and_execute' AND c.revoked_at IS NULL AND c.team_stale=0 AND c.authorization_version=? AND c.team_generation=? AND c.team_digest=? AND c.envelope_digest=? AND c.coordinator_task_role_id=? AND c.coordinator_actor_kind=? AND c.coordinator_actor_id=? AND c.requested_by=?)")
        .bind(task_id).bind(snapshot.task_version).bind(batch_id).bind(snapshot.task_title).bind(snapshot.task_description).bind(snapshot.task_repo_id).bind(snapshot.task_parent_task_id).bind(snapshot.authorization_version).bind(snapshot.team_generation).bind(snapshot.team_digest).bind(snapshot.envelope_digest).bind(snapshot.role_id).bind(snapshot.actor_kind).bind(snapshot.actor_id).bind(authorized_by).fetch_one(&mut **tx).await?)
}

pub(super) async fn batch_projection_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    batch_id: &str,
) -> Result<BatchProjection> {
    let row = sqlx::query("SELECT * FROM project_plan_batch WHERE id=? AND project_id=?")
        .bind(batch_id)
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServiceError::not_found("Project batch", batch_id.to_owned()))?;
    batch_projection_from_row(row)
}

pub(super) async fn batch_projection_by_id_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    batch_id: &str,
) -> Result<BatchProjection> {
    let row = sqlx::query("SELECT * FROM project_plan_batch WHERE id=?")
        .bind(batch_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| ServiceError::not_found("Project batch", batch_id.to_owned()))?;
    batch_projection_from_row(row)
}

fn batch_projection_from_row(row: sqlx::sqlite::SqliteRow) -> Result<BatchProjection> {
    Ok(BatchProjection {
        id: row.try_get("id")?,
        project_id: row.try_get("project_id")?,
        graph_version: row.try_get("graph_version")?,
        version: row.try_get("version")?,
        state: row.try_get("state")?,
        preparation_complete: row.try_get::<i64, _>("preparation_complete")? != 0,
        max_parallel_tasks: row.try_get("max_parallel_tasks")?,
        authorized_by: row.try_get("authorized_by")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

pub(super) async fn project_admin_current_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    user_id: &str,
) -> Result<bool> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project p JOIN project_member caller ON caller.project_id=p.id AND caller.user_id=? AND caller.role IN ('owner','admin') WHERE p.id=? AND p.owner_id IS NOT NULL AND EXISTS(SELECT 1 FROM project_member owner_membership WHERE owner_membership.project_id=p.id AND owner_membership.user_id=p.owner_id AND owner_membership.role='owner'))")
        .bind(user_id).bind(project_id).fetch_one(&mut **tx).await?)
}

async fn project_capacity_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    candidate_task_id: &str,
) -> Result<(i64, bool)> {
    let row=sqlx::query(
        "SELECT COUNT(DISTINCT occupied.task_id) AS occupied,
                MAX(CASE WHEN occupied.task_id=? THEN 1 ELSE 0 END) AS candidate_occupied
         FROM (
          SELECT t.id AS task_id FROM task t JOIN task_lifecycle l ON l.task_id=t.id
           WHERE t.project_id=? AND t.deleted_at IS NULL AND l.state='active'
          UNION
          SELECT bt.task_id FROM project_plan_batch_task bt JOIN project_plan_batch b ON b.id=bt.batch_id
           JOIN task t ON t.id=bt.task_id LEFT JOIN task_lifecycle l ON l.task_id=t.id
           WHERE b.project_id=? AND b.state='running' AND bt.state IN ('claimed','started')
             AND COALESCE(l.state,'')!='active'
         ) occupied",
    ).bind(candidate_task_id).bind(project_id).bind(project_id).fetch_one(&mut **tx).await?;
    Ok((
        row.try_get("occupied")?,
        row.try_get::<Option<i64>, _>("candidate_occupied")?
            .unwrap_or(0)
            != 0,
    ))
}

pub(super) fn coordination_blocker(error: &ServiceError) -> String {
    match error {
        ServiceError::Coordination { code, .. } => format!("coordination:{code}"),
        ServiceError::AuthorizationDenied { .. } => "coordination:authorization_revoked".into(),
        ServiceError::Db(
            db::DbError::TaskVersionConflict { .. } | db::DbError::VersionConflict,
        ) => "coordination:version_conflict".into(),
        ServiceError::Db(db::DbError::ProjectWriteRequired | db::DbError::ProjectAdminRequired) => {
            "coordination:project_membership_revoked".into()
        }
        ServiceError::Db(db::DbError::ProjectPlanApplyIncomplete) => {
            "coordination:project_plan_apply_incomplete".into()
        }
        ServiceError::Db(db::DbError::AgentPaused { .. }) => "coordination:agent_paused".into(),
        ServiceError::Db(db::DbError::AgentAtCapacity) => "coordination:agent_capacity".into(),
        _ => "coordination:authorization_failed".into(),
    }
}

async fn build_and_pin_handoffs_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    batch: &BatchProjection,
    task_id: &str,
) -> Result<Value> {
    let predecessors = sqlx::query_scalar::<_, String>(
        "SELECT depends_on_id FROM task_dependency WHERE task_id=? ORDER BY depends_on_id",
    )
    .bind(task_id)
    .fetch_all(&mut **tx)
    .await?;
    let mut pinned = Vec::with_capacity(predecessors.len());
    for predecessor_id in predecessors {
        let context = accepted_task_context_in_tx(
            tx,
            &batch.project_id,
            &predecessor_id,
            batch.graph_version,
        )
        .await?;
        let encoded = json_text(&context)?;
        let context_digest = digest(&context)?;
        sqlx::query("INSERT OR IGNORE INTO project_plan_handoff(batch_id,task_id,predecessor_task_id,context_json,digest,created_at) VALUES(?,?,?,?,?,?)")
            .bind(&batch.id).bind(task_id).bind(&predecessor_id).bind(&encoded).bind(&context_digest).bind(db::now_rfc3339()).execute(&mut **tx).await?;
        let stored=sqlx::query("SELECT context_json,digest FROM project_plan_handoff WHERE batch_id=? AND task_id=? AND predecessor_task_id=?")
            .bind(&batch.id).bind(task_id).bind(&predecessor_id).fetch_one(&mut **tx).await?;
        let stored_digest: String = stored.try_get("digest")?;
        if stored_digest != context_digest {
            return Err(ServiceError::Conflict(format!(
                "accepted predecessor {predecessor_id} changed after its handoff was pinned"
            )));
        }
        let stored_context: String = stored.try_get("context_json")?;
        pinned.push(decode::<Value>(&stored_context)?);
    }
    let context = json!({"direct_predecessors":pinned});
    if json_text(&context)?.len() > MAX_HANDOFF_CONTEXT_BYTES {
        return Err(ServiceError::Conflict(
            "exact predecessor handoff exceeds the supported coordinator context size".into(),
        ));
    }
    Ok(context)
}

fn handoff_blocker_code(message: &str) -> String {
    let code = if message.contains("not accepted") {
        "prerequisite_not_accepted"
    } else if message.contains("not fully integrated") {
        "prerequisite_not_integrated"
    } else if message.contains("unaccepted WorkUnit") {
        "work_unit_unaccepted"
    } else if message.contains("no exact completed Execution result") {
        "repository_execution_missing"
    } else if message.contains("no exact successful integration") {
        "integration_missing"
    } else if message.contains("does not bind an exact result commit") {
        "integration_result_missing"
    } else if message.contains("changed after its handoff was pinned") {
        "pinned_context_changed"
    } else if message.contains("context size") {
        "context_limit_exceeded"
    } else {
        "accepted_context_unavailable"
    };
    format!("handoff:{code}")
}

async fn accepted_task_context_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    project_id: &str,
    task_id: &str,
    graph_version: i64,
) -> Result<Value> {
    let task=sqlx::query("SELECT t.title,l.state FROM task t JOIN task_lifecycle l ON l.task_id=t.id WHERE t.id=? AND t.project_id=? AND t.deleted_at IS NULL")
        .bind(task_id).bind(project_id).fetch_optional(&mut **tx).await?
        .ok_or_else(||ServiceError::not_found("prerequisite Task",task_id.to_owned()))?;
    let title: String = task.try_get("title")?;
    let lifecycle: String = task.try_get("state")?;
    if lifecycle != "done" {
        return Err(ServiceError::Conflict(format!(
            "prerequisite Task {task_id} is not accepted"
        )));
    }
    let unsatisfied:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM project_task_dependency_readiness WHERE task_id=? AND satisfied=0)")
        .bind(task_id).fetch_one(&mut **tx).await?;
    if unsatisfied {
        return Err(ServiceError::Conflict(format!(
            "prerequisite Task {task_id} is not fully integrated"
        )));
    }
    let work_units=sqlx::query("SELECT id,version,title,status,requires_integration,provenance_kind,provenance_id FROM work_unit WHERE task_id=? ORDER BY created_at,id")
        .bind(task_id).fetch_all(&mut **tx).await?;
    let mut result_units = Vec::new();
    for unit in work_units {
        let work_unit_id: String = unit.try_get("id")?;
        let version: i64 = unit.try_get("version")?;
        let status: String = unit.try_get("status")?;
        let requires_integration: i64 = unit.try_get("requires_integration")?;
        let provenance_kind: Option<String> = unit.try_get("provenance_kind")?;
        let provenance_id: Option<String> = unit.try_get("provenance_id")?;
        if status == "cancelled" {
            result_units.push(json!({"work_unit_id":work_unit_id,"version":version,"status":status,"requires_integration":requires_integration!=0,"provenance":{"kind":provenance_kind,"id":provenance_id},"execution":null,"artifacts":[],"integration":null,"review":null}));
            continue;
        }
        if status != "completed" {
            return Err(ServiceError::Conflict(format!(
                "prerequisite Task {task_id} contains an unaccepted WorkUnit {work_unit_id}"
            )));
        }
        let expected_execution_version = version - 1;
        let execution=sqlx::query("SELECT id,work_unit_version,after_sha,workspace_id FROM execution WHERE task_id=? AND work_unit_id=? AND work_unit_version=? AND status='completed' ORDER BY updated_at DESC,id DESC LIMIT 1")
            .bind(task_id).bind(&work_unit_id).bind(expected_execution_version).fetch_optional(&mut **tx).await?;
        let execution_value = if let Some(execution) = execution {
            let execution_id: String = execution.try_get("id")?;
            let result_sha: Option<String> = execution.try_get("after_sha")?;
            let workspace_id: Option<String> = execution.try_get("workspace_id")?;
            let artifacts=sqlx::query("SELECT a.id,a.digest FROM artifact a JOIN artifact_execution_producer p ON p.artifact_id=a.id AND p.task_id=a.task_id WHERE p.execution_id=? AND a.task_id=? ORDER BY a.id")
                .bind(&execution_id).bind(task_id).fetch_all(&mut **tx).await?.into_iter().map(|row|Ok(json!({"artifact_id":row.try_get::<String,_>("id")?,"digest":row.try_get::<Option<String>,_>("digest")?}))).collect::<std::result::Result<Vec<_>,sqlx::Error>>()?;
            Some((execution_id, result_sha, workspace_id, artifacts))
        } else {
            None
        };
        if requires_integration != 0 && execution_value.is_none() {
            return Err(ServiceError::Conflict(format!(
                "repository WorkUnit {work_unit_id} has no exact completed Execution result"
            )));
        }
        let integration = if let Some((execution_id, source_sha, _, _)) = execution_value
            .as_ref()
            .filter(|_| requires_integration != 0)
        {
            let row=sqlx::query("SELECT i.id,i.target_after_sha,i.target_branch,target.repo_id,r.name FROM work_unit_integration i JOIN workspace target ON target.id=i.target_workspace_id JOIN repo r ON r.id=target.repo_id WHERE i.task_id=? AND i.work_unit_id=? AND i.execution_id=? AND i.outcome='success' ORDER BY i.finished_at DESC,i.id DESC LIMIT 1")
                .bind(task_id).bind(&work_unit_id).bind(execution_id).fetch_optional(&mut **tx).await?
                .ok_or_else(||ServiceError::Conflict(format!("repository WorkUnit {work_unit_id} has no exact successful integration")))?;
            let target_after_sha: Option<String> = row.try_get("target_after_sha")?;
            if target_after_sha.is_none() || source_sha.is_none() {
                return Err(ServiceError::Conflict(format!("repository WorkUnit {work_unit_id} integration does not bind an exact result commit")));
            }
            Some(
                json!({"integration_id":row.try_get::<String,_>("id")?,"repo_id":row.try_get::<String,_>("repo_id")?,"repo_name":row.try_get::<String,_>("name")?,"target_branch":row.try_get::<String,_>("target_branch")?,"commit_sha":target_after_sha}),
            )
        } else {
            None
        };
        let execution_json=execution_value.as_ref().map(|(execution_id,sha,workspace,_)|json!({"execution_id":execution_id,"result_commit_sha":sha,"workspace_id":workspace}));
        let artifacts = execution_value
            .as_ref()
            .map(|(_, _, _, artifacts)| artifacts.clone())
            .unwrap_or_default();
        let review=sqlx::query("SELECT id,revision,latest_report_artifact_id,latest_verdict,current_result_artifact_id,current_result_execution_id,current_commit_sha,current_workspace_id,current_snapshot_digest FROM work_unit_review_line WHERE task_id=? AND current_work_unit_id=? AND current_work_unit_version=? ORDER BY revision DESC,id DESC LIMIT 1")
            .bind(task_id).bind(&work_unit_id).bind(expected_execution_version).fetch_optional(&mut **tx).await?
            .map(|row| -> std::result::Result<Value, sqlx::Error> {
                Ok(json!({"review_line_id":row.try_get::<String,_>("id")?,"revision":row.try_get::<i64,_>("revision")?,"report_artifact_id":row.try_get::<Option<String>,_>("latest_report_artifact_id")?,"verdict":row.try_get::<Option<String>,_>("latest_verdict")?,"result_artifact_id":row.try_get::<Option<String>,_>("current_result_artifact_id")?,"result_execution_id":row.try_get::<Option<String>,_>("current_result_execution_id")?,"result_commit_sha":row.try_get::<Option<String>,_>("current_commit_sha")?,"workspace_id":row.try_get::<Option<String>,_>("current_workspace_id")?,"snapshot_digest":row.try_get::<Option<String>,_>("current_snapshot_digest")?}))
            })
            .transpose()?;
        result_units.push(json!({"work_unit_id":work_unit_id,"version":version,"status":status,"requires_integration":requires_integration!=0,"provenance":{"kind":provenance_kind,"id":provenance_id},"execution":execution_json,"artifacts":artifacts,"integration":integration,"review":review}));
    }
    Ok(
        json!({"task_id":task_id,"title":title,"lifecycle_state":lifecycle,"accepted_at_graph_version":graph_version,"work_units":result_units}),
    )
}
