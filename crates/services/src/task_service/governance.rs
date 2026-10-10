//! Task and WorkspaceLease admission using V2 Task-scoped authority.

use super::*;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};
use sqlx::{Row, Sqlite, Transaction};

const READ_ONLY_CAPABILITY_TYPES: &[&str] = &["planning", "discovery", "review", "validation"];
const WORKSPACE_LEASE_SECONDS: i64 = 15 * 60;
const CAPABILITY_PROFILE_REVISION: &str = "forge.capability-profile/v1";

/// Resolve the capability axis independently from TaskRole and lease class.
fn workspace_lease_capability_class(
    task_type: &str,
    purpose: Option<&str>,
    execution_role: &str,
) -> &'static str {
    let read_only = execution_role.eq_ignore_ascii_case("reviewer")
        || matches!(
            purpose,
            Some("plan" | "review" | "investigate" | "validate")
        )
        || READ_ONLY_CAPABILITY_TYPES.contains(&task_type);
    if read_only {
        "repository_read"
    } else {
        "repository_write"
    }
}

impl TaskService {
    /// Prepare the exact scheduler-issued authority for a repository-mutating
    /// WorkUnit Execution. The lease row is inserted by the same DB
    /// transaction that creates the Execution and its durable event.
    pub(crate) async fn prepare_work_unit_workspace_lease(
        &self,
        task: &db::Task,
        work_unit: &db::WorkUnit,
        workspace: &db::Workspace,
        execution: &db::CreateExecution,
        actor: &db::ActorRef,
    ) -> Result<CreateWorkspaceLease> {
        let repo_id = task.repo_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation(
                "repository-mutating WorkUnit requires a repository-bound Task",
            )
        })?;
        if work_unit.task_id != task.id
            || work_unit.role != execution.role
            || execution.workspace_id.as_deref() != Some(workspace.id.as_str())
            || execution.actor_ref.as_ref() != Some(actor)
        {
            return Err(ServiceError::invalid_operation(
                "WorkUnit, Execution, Actor, and Workspace lease bindings do not match",
            ));
        }
        self.ensure_task_runnable(task).await?;
        let canonical_role = canonical_workspace_lease_role(&work_unit.role)?;
        let db::ActorRef::Agent(principal_id) = actor else {
            return Err(ServiceError::invalid_operation(
                "repository WorkUnit leases require an Agent Actor under current repository policy",
            ));
        };
        if execution.agent_id.as_deref() != Some(principal_id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "repository WorkUnit Execution agent_id must match its Agent Actor",
            ));
        }
        let target_role = db::canonical_task_role_name(work_unit.role.trim())
            .ok_or_else(|| ServiceError::invalid_operation("WorkUnit role is not a TaskRole"))?;
        let membership_exists: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_role tr
                JOIN role_membership rm ON rm.task_role_id = tr.id
                WHERE tr.task_id = ? AND tr.role = ?
                  AND rm.actor_kind = ? AND rm.actor_id = ? AND rm.status = 'active'
            )",
        )
        .bind(&task.id)
        .bind(&target_role)
        .bind(actor.kind().to_string())
        .bind(actor.id())
        .fetch_one(self.db.pool())
        .await?;
        if membership_exists == 0 {
            return Err(ServiceError::conflict(
                "WorkUnit WorkspaceLease requires an active TaskRole membership",
            ));
        }
        self.ensure_repository_worker_identity(&task.project_id, principal_id)
            .await?;
        let (_repo, capability_class, base_ref) = self
            .workspace_lease_inputs(
                task,
                workspace,
                repo_id,
                &execution.id,
                Some(
                    execution
                        .purpose
                        .clone()
                        .unwrap_or(db::ExecutionPurpose::General),
                ),
                &work_unit.role,
            )
            .await?;
        let issued_at = now_rfc3339();
        let expires_at =
            (Utc::now() + ChronoDuration::seconds(WORKSPACE_LEASE_SECONDS)).to_rfc3339();
        let capabilities_json = serde_json::to_string(std::slice::from_ref(&capability_class))
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        Ok(CreateWorkspaceLease {
            id: new_uuid_v4(),
            project_id: task.project_id.clone(),
            task_id: task.id.clone(),
            work_unit_id: Some(work_unit.id.clone()),
            workspace_id: Some(workspace.id.clone()),
            task_version: task.version,
            execution_id: execution.id.clone(),
            operation_idempotency_key: execution.id.clone(),
            repository_binding_id: repo_id.to_owned(),
            base_ref,
            role: canonical_role.to_owned(),
            capabilities_json,
            assigned_principal_type: "agent".to_owned(),
            assigned_principal_id: principal_id.to_owned(),
            capability_profile_revision: CAPABILITY_PROFILE_REVISION.to_owned(),
            capability_profile_digest: capability_profile_digest(&capability_class),
            issuing_principal_type: "system".to_owned(),
            issuing_principal_id: "task-service-scheduler".to_owned(),
            issued_at: issued_at.clone(),
            expires_at,
            created_at: issued_at.clone(),
            updated_at: issued_at,
        })
    }

    /// Check generic Project eligibility before any repository workspace is
    /// prepared. The in-transaction lease guard repeats this at the authority
    /// boundary so bindings cannot substitute for Project ownership/member or
    /// global Agent eligibility.
    pub(super) async fn ensure_repository_worker_identity(
        &self,
        project_id: &str,
        principal_id: &str,
    ) -> Result<()> {
        if !crate::task_service::repository_worker_identity_is_eligible(
            &self.db,
            project_id,
            principal_id,
        )
        .await?
        {
            return Err(ServiceError::invalid_operation(
                "Agent must be globally visible or owned by a Project owner/member to receive a repository WorkspaceLease",
            ));
        }
        Ok(())
    }

    pub(super) async fn ensure_task_runnable(&self, task: &db::Task) -> Result<()> {
        if crate::task_failure_retry::TaskFailureRetryService::has_exhausted_retry_budget(
            &self.db, &task.id,
        )
        .await?
        {
            return Err(ServiceError::invalid_operation(
                "Task retry budget is exhausted; further Execution dispatch is blocked",
            ));
        }
        Ok(())
    }

    pub(super) async fn issue_workspace_lease(
        &self,
        task: &db::Task,
        workspace: &db::Workspace,
        role: &str,
        principal_id: Option<&str>,
        execution_id: &str,
    ) -> Result<Option<db::WorkspaceLease>> {
        let Some(repo_id) = task.repo_id.as_deref() else {
            return Ok(None);
        };
        self.ensure_task_runnable(task).await?;
        let canonical_role = canonical_workspace_lease_role(role)?;
        let principal_id = self
            .validate_workspace_assignment(task, role, principal_id)
            .await?;
        let (_repo, capability_class, base_ref) = self
            .workspace_lease_inputs(task, workspace, repo_id, execution_id, None, role)
            .await?;

        // A lease is reusable only while every binding remains exact.  This
        // also closes the race where two launchers observe no lease and one
        // of them inserts an authority row after the other has already done
        // so: the unique active-task constraint plus the verification below
        // make the winner authoritative and the loser fail closed.
        if let Some(existing) = WorkspaceLeaseRepo::get_active_for_task(&*self.db, &task.id).await?
        {
            if !workspace_lease_expired(&existing) {
                return self
                    .verify_active_workspace_lease(
                        task,
                        workspace,
                        role,
                        Some(&principal_id),
                        execution_id,
                    )
                    .await
                    .map(Some);
            }
            if let Err(error) = WorkspaceLeaseRepo::expire(&*self.db, &now_rfc3339(), 500).await {
                tracing::warn!(lease_id = %existing.id, %error, "failed to expire stale WorkspaceLease before reissue");
            }
        }

        let issued_at = now_rfc3339();
        let expires_at =
            (Utc::now() + ChronoDuration::seconds(WORKSPACE_LEASE_SECONDS)).to_rfc3339();
        let capabilities_json = serde_json::to_string(std::slice::from_ref(&capability_class))
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let input = CreateWorkspaceLease {
            id: new_uuid_v4(),
            project_id: task.project_id.clone(),
            task_id: task.id.clone(),
            work_unit_id: None,
            workspace_id: None,
            task_version: task.version,
            execution_id: execution_id.to_owned(),
            operation_idempotency_key: execution_id.to_owned(),
            repository_binding_id: repo_id.to_owned(),
            base_ref,
            role: canonical_role.to_owned(),
            capabilities_json,
            assigned_principal_type: "agent".to_owned(),
            assigned_principal_id: principal_id.clone(),
            capability_profile_revision: CAPABILITY_PROFILE_REVISION.to_owned(),
            capability_profile_digest: capability_profile_digest(&capability_class),
            // The issuer is always the internal scheduler.  The assigned
            // worker/reviewer is checked separately and is never exposed as
            // a bearer token or chat-visible lease field.
            issuing_principal_type: "system".to_owned(),
            issuing_principal_id: "task-service-scheduler".to_owned(),
            issued_at: issued_at.clone(),
            expires_at,
            created_at: issued_at.clone(),
            updated_at: issued_at,
        };
        let _lease = match WorkspaceLeaseRepo::issue(&*self.db, input).await {
            Ok(lease) => lease,
            Err(error) => {
                // Another scheduler may have won the active-task race.  Only
                // accept its row after rechecking all bindings; otherwise the
                // insert error remains a hard admission failure.
                if WorkspaceLeaseRepo::get_active_for_task(&*self.db, &task.id)
                    .await?
                    .is_some()
                {
                    return self
                        .verify_active_workspace_lease(
                            task,
                            workspace,
                            role,
                            Some(&principal_id),
                            execution_id,
                        )
                        .await
                        .map(Some);
                }
                return Err(error.into());
            }
        };
        self.verify_active_workspace_lease(task, workspace, role, Some(&principal_id), execution_id)
            .await
            .map(Some)
    }

    /// Issue a lease while the task claim transaction is still open.  The
    /// TaskRepo claim updates assignment and creates the Running execution in
    /// the same transaction; this insert therefore cannot leave an authority
    /// for an unassigned Task after a process crash.
    pub(super) async fn issue_workspace_lease_in_tx(
        &self,
        transaction: &mut Transaction<'_, Sqlite>,
        task: &db::Task,
        workspace: &db::Workspace,
        role: &str,
        principal_id: Option<&str>,
        execution_id: &str,
    ) -> Result<db::WorkspaceLease> {
        let repo_id = task.repo_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("WorkspaceLease requires a repository-backed Task")
        })?;
        let canonical_role = canonical_workspace_lease_role(role)?;
        let task_role = db::canonical_task_role_name(role.trim())
            .or_else(|| {
                role.trim()
                    .eq_ignore_ascii_case("interactive")
                    .then(|| {
                        crate::task_service::execution::task_role_for_task_type(&task.task_type)
                            .to_owned()
                    })
                    .and_then(|role| db::canonical_task_role_name(&role))
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation("WorkspaceLease role is not a TaskRole")
            })?;
        let role_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
        )
        .bind(&task.id)
        .bind(task_role)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("WorkspaceLease requires an explicit TaskRole")
        })?;
        let members = sqlx::query(
            "SELECT actor_id FROM role_membership
             WHERE task_role_id = ? AND actor_kind = 'agent' AND status = 'active'
             ORDER BY created_at, id",
        )
        .bind(&role_id)
        .fetch_all(&mut **transaction)
        .await?;
        let principal_id =
            if let Some(principal_id) = principal_id.filter(|id| !id.trim().is_empty()) {
                if !members
                    .iter()
                    .any(|member| member.get::<String, _>("actor_id") == principal_id)
                {
                    return Err(ServiceError::conflict(
                        "WorkspaceLease principal is not an active TaskRole member",
                    ));
                }
                principal_id.to_owned()
            } else if members.len() == 1 {
                members[0].get::<String, _>("actor_id")
            } else {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceLease requires one concrete active Agent TaskRole member",
                ));
            };
        let eligible: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM agent_current a JOIN project p ON p.id = ?
                WHERE a.id = ? AND (
                    a.visibility = 'global'
                    OR (a.visibility = 'account' AND a.owner_id IS NOT NULL AND (
                        a.owner_id = p.owner_id
                        OR EXISTS (SELECT 1 FROM project_member pm
                                   WHERE pm.project_id = p.id AND pm.user_id = a.owner_id)
                    ))
                )
            )",
        )
        .bind(&task.project_id)
        .bind(&principal_id)
        .fetch_one(&mut **transaction)
        .await?;
        if eligible == 0 {
            return Err(ServiceError::invalid_operation(
                "WorkspaceLease Agent is not eligible for this Project",
            ));
        }
        let task_row = sqlx::query(
            "SELECT t.project_id, t.repo_id, t.task_type, r.default_branch
             FROM task t JOIN repo r ON r.id = t.repo_id
             WHERE t.id = ? AND t.project_id = ? AND t.deleted_at IS NULL",
        )
        .bind(&task.id)
        .bind(&task.project_id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| ServiceError::not_found("task", task.id.clone()))?;
        let bound_repo_id: String = task_row.get("repo_id");
        let task_type: String = task_row.get("task_type");
        let default_branch: String = task_row.get("default_branch");
        if bound_repo_id != repo_id || workspace.repo_id != repo_id {
            return Err(ServiceError::invalid_operation(
                "workspace repository does not match the Task repository binding",
            ));
        }
        let execution = sqlx::query(
            "SELECT actor_kind, actor_id, agent_id, role, purpose, status
             FROM execution WHERE id = ? AND task_id = ?",
        )
        .bind(execution_id)
        .bind(&task.id)
        .fetch_optional(&mut **transaction)
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("WorkspaceLease requires an exact Task Execution")
        })?;
        if execution.get::<String, _>("actor_kind") != "agent"
            || execution.get::<String, _>("actor_id") != principal_id
            || execution.get::<Option<String>, _>("agent_id").as_deref()
                != Some(principal_id.as_str())
            || execution.get::<String, _>("status") != "running"
        {
            return Err(ServiceError::invalid_operation(
                "WorkspaceLease principal must match the running Execution Actor",
            ));
        }
        let purpose: Option<String> = execution.get("purpose");
        let execution_role: String = execution.get("role");
        let capability_class =
            workspace_lease_capability_class(&task_type, purpose.as_deref(), &execution_role);
        let base_ref = workspace.before_sha.clone().unwrap_or(default_branch);
        let issued_at = now_rfc3339();
        let expires_at =
            (Utc::now() + ChronoDuration::seconds(WORKSPACE_LEASE_SECONDS)).to_rfc3339();
        let capabilities_json = serde_json::to_string(&[capability_class])
            .map_err(|error| ServiceError::invalid_operation(error.to_string()))?;
        let lease_id = new_uuid_v4();
        sqlx::query(
            "INSERT INTO workspace_lease (
                id, project_id, task_id, task_version, execution_id,
                operation_idempotency_key, repository_binding_id, base_ref, role,
                capabilities_json, assigned_principal_type, assigned_principal_id,
                capability_profile_revision, capability_profile_digest,
                issuing_principal_type, issuing_principal_id, status, issued_at,
                expires_at, revoked_at, version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'agent', ?, ?, ?,
                       'system', 'task-service-scheduler', 'active', ?, ?, NULL, 1, ?, ?)",
        )
        .bind(&lease_id)
        .bind(&task.project_id)
        .bind(&task.id)
        .bind(task.version)
        .bind(execution_id)
        .bind(execution_id)
        .bind(repo_id)
        .bind(&base_ref)
        .bind(canonical_role)
        .bind(&capabilities_json)
        .bind(&principal_id)
        .bind(CAPABILITY_PROFILE_REVISION)
        .bind(capability_profile_digest(capability_class))
        .bind(&issued_at)
        .bind(&expires_at)
        .bind(&issued_at)
        .bind(&issued_at)
        .execute(&mut **transaction)
        .await?;
        let row = sqlx::query("SELECT * FROM workspace_lease WHERE id = ?")
            .bind(&lease_id)
            .fetch_one(&mut **transaction)
            .await?;
        Ok(map_workspace_lease_row(row))
    }

    pub(super) async fn verify_active_workspace_lease(
        &self,
        task: &db::Task,
        workspace: &db::Workspace,
        role: &str,
        principal_id: Option<&str>,
        execution_id: &str,
    ) -> Result<db::WorkspaceLease> {
        let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace", workspace.id.clone()))?;
        if scope.task_id != task.id {
            return Err(ServiceError::not_found("workspace", workspace.id.clone()));
        }
        if scope.kind == db::WorkspaceScopeKind::WorkUnit {
            return self
                .verify_active_work_unit_workspace_lease(
                    task,
                    workspace,
                    scope.work_unit_id.as_deref().ok_or_else(|| {
                        ServiceError::invalid_operation(
                            "WorkUnit Workspace has no WorkUnit identity",
                        )
                    })?,
                    role,
                    principal_id,
                    execution_id,
                )
                .await;
        }
        if !WorkUnitRepo::list_by_task(&*self.db, &task.id)
            .await?
            .is_empty()
        {
            return Err(ServiceError::invalid_operation(
                "Task integration Workspace cannot authorize a WorkUnit Execution",
            ));
        }
        let repo_id = task.repo_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("WorkspaceLease requires a repository-backed Task")
        })?;
        self.ensure_task_runnable(task).await?;
        let canonical_role = canonical_workspace_lease_role(role)?;
        let principal_id = self
            .validate_workspace_assignment(task, role, principal_id)
            .await?;
        let (repo, capability_class, base_ref) = self
            .workspace_lease_inputs(task, workspace, repo_id, execution_id, None, role)
            .await?;
        let lease = WorkspaceLeaseRepo::get_active_for_task(&*self.db, &task.id)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "repository execution requires an active scheduler WorkspaceLease",
                )
            })?;
        if workspace_lease_expired(&lease) {
            if let Err(error) = WorkspaceLeaseRepo::expire(&*self.db, &now_rfc3339(), 500).await {
                tracing::warn!(lease_id = %lease.id, %error, "failed to expire stale WorkspaceLease");
            }
            return Err(ServiceError::invalid_operation(
                "scheduler WorkspaceLease has expired",
            ));
        }
        let capabilities =
            serde_json::from_str::<Vec<String>>(&lease.capabilities_json).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "invalid WorkspaceLease capability set: {error}"
                ))
            })?;
        // `task_version` records the revision admitted when the lease was
        // issued. Task edits such as comments or descriptions are not
        // authority changes and must not revoke a running execution. The
        // assignment, repository, capability, lifecycle, and execution
        // bindings below remain fail-closed authority checks.
        if lease.status != "active"
            || lease.project_id != task.project_id
            || lease.task_id != task.id
            || lease.execution_id != execution_id
            || lease.operation_idempotency_key != execution_id
            || lease.repository_binding_id != repo_id
            || lease.base_ref != base_ref
            || lease.role != canonical_role
            || lease.issuing_principal_type != "system"
            || lease.issuing_principal_id != "task-service-scheduler"
            || lease.assigned_principal_type != "agent"
            || lease.assigned_principal_id != principal_id
            || lease.capability_profile_revision != CAPABILITY_PROFILE_REVISION
            || lease.capability_profile_digest != capability_profile_digest(&capability_class)
            || capabilities != vec![capability_class]
            || repo.project_id != task.project_id
        {
            return Err(ServiceError::invalid_operation(
                "active WorkspaceLease does not exactly match Task execution authority",
            ));
        }
        Ok(lease)
    }

    async fn verify_active_work_unit_workspace_lease(
        &self,
        task: &db::Task,
        workspace: &db::Workspace,
        work_unit_id: &str,
        role: &str,
        principal_id: Option<&str>,
        execution_id: &str,
    ) -> Result<db::WorkspaceLease> {
        let unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .filter(|unit| unit.task_id == task.id)
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
            .await?
            .filter(|execution| execution.task_id == task.id)
            .ok_or_else(|| ServiceError::not_found("execution", execution_id.to_owned()))?;
        let role_name = db::canonical_task_role_name(&unit.role)
            .ok_or_else(|| ServiceError::invalid_operation("WorkUnit role is not a TaskRole"))?;
        let db::ActorRef::Agent(agent_id) = execution
            .actor_ref()
            .ok_or_else(|| ServiceError::invalid_operation("Execution has no ActorRef"))?
        else {
            return Err(ServiceError::invalid_operation(
                "repository WorkUnit WorkspaceLease requires an Agent Actor",
            ));
        };
        if !unit.requires_integration
            || unit.status != db::WorkUnitStatus::Open
            || unit.role != role
            || execution.work_unit_id.as_deref() != Some(work_unit_id)
            || execution.work_unit_version != Some(unit.version)
            || execution.status != ExecutionStatus::Running
            || execution.role != unit.role
            || execution.workspace_id.as_deref() != Some(workspace.id.as_str())
            || execution.agent_id.as_deref() != Some(agent_id.as_str())
            || principal_id.is_some_and(|principal| principal != agent_id)
            || unit
                .assigned_actor
                .as_ref()
                .is_some_and(|assigned| assigned != &db::ActorRef::Agent(agent_id.clone()))
            || workspace.status != WorkspaceStatus::Ready
        {
            return Err(ServiceError::invalid_operation(
                "WorkUnit Execution, allocation, Workspace, and lease principal do not match",
            ));
        }
        self.ensure_task_runnable(task).await?;
        self.ensure_repository_worker_identity(&task.project_id, &agent_id)
            .await?;
        let member: i64 = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM task_role tr JOIN role_membership rm ON rm.task_role_id = tr.id
                WHERE tr.task_id = ? AND tr.role = ?
                  AND rm.actor_kind = 'agent' AND rm.actor_id = ? AND rm.status = 'active'
            )",
        )
        .bind(&task.id)
        .bind(&role_name)
        .bind(&agent_id)
        .fetch_one(self.db.pool())
        .await?;
        if member == 0 {
            return Err(ServiceError::conflict(
                "WorkUnit WorkspaceLease requires an active TaskRole membership",
            ));
        }

        let repo_id = task.repo_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("WorkUnit WorkspaceLease requires a repository Task")
        })?;
        let (repo, capability_class, base_ref) = self
            .workspace_lease_inputs(task, workspace, repo_id, execution_id, None, &unit.role)
            .await?;
        if repo.project_id != task.project_id {
            return Err(ServiceError::invalid_operation(
                "WorkUnit WorkspaceLease repository belongs to another Project",
            ));
        }
        let lease = WorkspaceLeaseRepo::get_active_for_work_unit(&*self.db, work_unit_id)
            .await?
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "WorkUnit repository Execution requires an active WorkspaceLease",
                )
            })?;
        if workspace_lease_expired(&lease) {
            if let Err(error) = WorkspaceLeaseRepo::expire(&*self.db, &now_rfc3339(), 500).await {
                tracing::warn!(lease_id = %lease.id, %error, "failed to expire stale WorkUnit WorkspaceLease");
            }
            return Err(ServiceError::invalid_operation(
                "WorkUnit WorkspaceLease has expired",
            ));
        }
        let capabilities =
            serde_json::from_str::<Vec<String>>(&lease.capabilities_json).map_err(|error| {
                ServiceError::invalid_operation(format!(
                    "invalid WorkUnit WorkspaceLease capability set: {error}"
                ))
            })?;
        let canonical_role = canonical_workspace_lease_role(&unit.role)?;
        if lease.status != "active"
            || lease.project_id != task.project_id
            || lease.task_id != task.id
            || lease.work_unit_id.as_deref() != Some(work_unit_id)
            || lease.workspace_id.as_deref() != Some(workspace.id.as_str())
            || lease.execution_id != execution_id
            || lease.operation_idempotency_key != execution_id
            || lease.repository_binding_id != repo_id
            || lease.base_ref != base_ref
            || lease.role != canonical_role
            || lease.issuing_principal_type != "system"
            || lease.issuing_principal_id != "task-service-scheduler"
            || lease.assigned_principal_type != "agent"
            || lease.assigned_principal_id != agent_id
            || lease.capability_profile_revision != CAPABILITY_PROFILE_REVISION
            || lease.capability_profile_digest != capability_profile_digest(&capability_class)
            || capabilities != vec![capability_class]
        {
            return Err(ServiceError::invalid_operation(
                "active WorkspaceLease does not exactly match WorkUnit Execution authority",
            ));
        }
        Ok(lease)
    }

    async fn workspace_lease_inputs(
        &self,
        task: &db::Task,
        workspace: &db::Workspace,
        repo_id: &str,
        execution_id: &str,
        purpose: Option<db::ExecutionPurpose>,
        role: &str,
    ) -> Result<(db::Repo, String, String)> {
        if workspace.repo_id != repo_id {
            return Err(ServiceError::invalid_operation(
                "workspace repository does not match the Task repository binding",
            ));
        }
        let repo = RepoRepo::get_by_id(&*self.db, repo_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("repo", repo_id.to_owned()))?;
        if repo.project_id != task.project_id {
            return Err(ServiceError::invalid_operation(
                "Task repository binding belongs to a different Project",
            ));
        }
        let purpose = if let Some(purpose) = purpose {
            purpose
        } else {
            let execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                .await?
                .filter(|execution| execution.task_id == task.id)
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "WorkspaceLease requires an exact Task Execution",
                    )
                })?;
            execution.purpose.unwrap_or(db::ExecutionPurpose::General)
        };
        let purpose = purpose.to_string();
        let capability_class =
            workspace_lease_capability_class(&task.task_type, Some(&purpose), role).to_owned();
        if !is_supported_capability_profile(&capability_class) {
            return Err(ServiceError::invalid_operation(format!(
                "Task capability profile '{}' is not server-approved",
                capability_class
            )));
        }
        let base_ref = workspace
            .before_sha
            .clone()
            .unwrap_or_else(|| repo.default_branch.clone());
        Ok((repo, capability_class, base_ref))
    }

    async fn validate_workspace_assignment(
        &self,
        task: &db::Task,
        role: &str,
        principal_id: Option<&str>,
    ) -> Result<String> {
        let target_role = db::canonical_task_role_name(role.trim())
            .or_else(|| {
                role.trim()
                    .eq_ignore_ascii_case("interactive")
                    .then(|| {
                        crate::task_service::execution::task_role_for_task_type(&task.task_type)
                            .to_owned()
                    })
                    .and_then(|role| db::canonical_task_role_name(&role))
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation("WorkspaceLease role is not a TaskRole")
            })?;
        let role_id = sqlx::query_scalar::<_, String>(
            "SELECT id FROM task_role WHERE task_id = ? AND role = ?",
        )
        .bind(&task.id)
        .bind(target_role)
        .fetch_optional(self.db.pool())
        .await?
        .ok_or_else(|| {
            ServiceError::invalid_operation("WorkspaceLease requires an explicit TaskRole")
        })?;
        let members = sqlx::query(
            "SELECT actor_id FROM role_membership
             WHERE task_role_id = ? AND actor_kind = 'agent' AND status = 'active'
             ORDER BY created_at, id",
        )
        .bind(role_id)
        .fetch_all(self.db.pool())
        .await?;
        let principal_id =
            if let Some(principal_id) = principal_id.filter(|id| !id.trim().is_empty()) {
                if !members
                    .iter()
                    .any(|member| member.get::<String, _>("actor_id") == principal_id)
                {
                    return Err(ServiceError::conflict(
                        "WorkspaceLease principal is not an active TaskRole member",
                    ));
                }
                principal_id.to_owned()
            } else if members.len() == 1 {
                members[0].get::<String, _>("actor_id")
            } else {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceLease requires one concrete active Agent TaskRole member",
                ));
            };
        self.ensure_repository_worker_identity(&task.project_id, &principal_id)
            .await?;
        Ok(principal_id)
    }

    pub(super) async fn revoke_active_workspace_lease_for_execution(
        &self,
        task_id: &str,
        execution_id: &str,
    ) {
        let execution = match ExecutionRepo::get_by_id(&*self.db, execution_id).await {
            Ok(Some(execution)) if execution.task_id == task_id => execution,
            Ok(Some(_)) | Ok(None) => return,
            Err(error) => {
                tracing::warn!(%error, task_id, execution_id, "failed to load Execution lease binding");
                return;
            }
        };
        let active_lease = match execution.work_unit_id.as_deref() {
            Some(work_unit_id) => {
                WorkspaceLeaseRepo::get_active_for_work_unit(&*self.db, work_unit_id).await
            }
            None => WorkspaceLeaseRepo::get_active_for_task(&*self.db, task_id).await,
        };
        match active_lease {
            Ok(Some(lease)) if lease.execution_id == execution_id => {
                self.revoke_workspace_lease(&lease).await
            }
            Ok(Some(lease)) => {
                // A concurrent retry may already own this WorkUnit's next
                // lease. Never revoke another Execution's authority while
                // terminalizing this attempt.
                tracing::debug!(
                    task_id,
                    execution_id,
                    active_execution_id = %lease.execution_id,
                    "leaving another execution's WorkspaceLease active"
                );
            }
            Ok(None) => {}
            Err(error) => tracing::warn!(%error, task_id, "failed to load terminal WorkspaceLease"),
        }
    }

    pub(super) async fn verify_execution_workspace_authority(
        &self,
        execution: &db::Execution,
    ) -> Result<Option<db::WorkspaceLease>> {
        if execution.role == "orchestrator"
            && execution.purpose == Some(db::ExecutionPurpose::Orchestrate)
        {
            if execution.workspace_id.is_some() {
                return Err(ServiceError::invalid_operation(
                    "orchestrator Execution cannot receive a mutable WorkspaceLease",
                ));
            }
            self.validate_orchestrator_execution(execution).await?;
            return Ok(None);
        }
        let task = TaskRepo::get_by_id(&*self.db, &execution.task_id, false)
            .await?
            .ok_or_else(|| ServiceError::not_found("task", execution.task_id.clone()))?;
        let Some(workspace_id) = execution.workspace_id.as_deref() else {
            if task.repo_id.is_some() {
                return Err(ServiceError::invalid_operation(
                    "repository execution requires a scheduler WorkspaceLease-backed workspace",
                ));
            }
            return Ok(None);
        };
        let workspace = WorkspaceRepo::get_by_id(&*self.db, workspace_id)
            .await?
            .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
        self.verify_active_workspace_lease(
            &task,
            &workspace,
            &execution.role,
            execution.agent_id.as_deref(),
            &execution.id,
        )
        .await
        .map(Some)
    }

    pub(super) async fn revoke_workspace_lease(&self, lease: &db::WorkspaceLease) {
        if let Err(error) =
            WorkspaceLeaseRepo::revoke(&*self.db, &lease.id, lease.version, &now_rfc3339()).await
        {
            tracing::warn!(
                lease_id = %lease.id,
                %error,
                "failed to revoke WorkspaceLease after execution admission failure"
            );
        }
    }
}

fn canonical_workspace_lease_role(role: &str) -> Result<&'static str> {
    match role.trim() {
        "reviewer" => Ok("reviewer"),
        // Workflow role names are user-configurable. Every scheduler-resolved
        // execution role other than the dedicated reviewer role is a bounded
        // Task Worker for lease purposes; the exact original role still has
        // to match the authoritative Task role assignment.
        _ => Ok("worker"),
    }
}

fn workspace_lease_expired(lease: &db::WorkspaceLease) -> bool {
    DateTime::parse_from_rfc3339(&lease.expires_at)
        .map(|expires_at| expires_at.with_timezone(&Utc) <= Utc::now())
        .unwrap_or(true)
}

fn capability_profile_digest(capability_class: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(CAPABILITY_PROFILE_REVISION.as_bytes());
    digest.update([0]);
    digest.update(capability_class.as_bytes());
    format!("sha256:{}", hex::encode(digest.finalize()))
}

fn is_supported_capability_profile(capability_class: &str) -> bool {
    matches!(
        capability_class,
        "repository_read" | "repository_write" | "read_only" | "discovery_read" | "planning_read"
    )
}

fn map_workspace_lease_row(row: sqlx::sqlite::SqliteRow) -> db::WorkspaceLease {
    db::WorkspaceLease {
        id: row.get("id"),
        project_id: row.get("project_id"),
        task_id: row.get("task_id"),
        work_unit_id: row.get("work_unit_id"),
        workspace_id: row.get("workspace_id"),
        task_version: row.get("task_version"),
        execution_id: row.get("execution_id"),
        operation_idempotency_key: row.get("operation_idempotency_key"),
        repository_binding_id: row.get("repository_binding_id"),
        base_ref: row.get("base_ref"),
        role: row.get("role"),
        capabilities_json: row.get("capabilities_json"),
        assigned_principal_type: row.get("assigned_principal_type"),
        assigned_principal_id: row.get("assigned_principal_id"),
        capability_profile_revision: row.get("capability_profile_revision"),
        capability_profile_digest: row.get("capability_profile_digest"),
        issuing_principal_type: row.get("issuing_principal_type"),
        issuing_principal_id: row.get("issuing_principal_id"),
        status: row.get("status"),
        issued_at: row.get("issued_at"),
        expires_at: row.get("expires_at"),
        revoked_at: row.get("revoked_at"),
        version: row.get("version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_lease_roles_preserve_reviewer_and_bound_custom_workers() {
        assert_eq!(
            canonical_workspace_lease_role("reviewer").expect("reviewer role"),
            "reviewer"
        );
        assert_eq!(
            canonical_workspace_lease_role("implementer").expect("custom worker role"),
            "worker"
        );
        assert_eq!(
            canonical_workspace_lease_role("orchestrator").expect("workflow worker role"),
            "worker"
        );
    }

    #[test]
    fn task_role_execution_purpose_lease_class_and_capability_stay_independent() {
        struct Case {
            execution_role: &'static str,
            task_type: &'static str,
            task_role: &'static str,
            lease_role: &'static str,
            purpose: &'static str,
            capability: &'static str,
        }
        let cases = [
            Case {
                execution_role: "implementer",
                task_type: "implementation",
                task_role: "implementer",
                lease_role: "worker",
                purpose: "implement",
                capability: "repository_write",
            },
            Case {
                execution_role: "planner",
                task_type: "planning",
                task_role: "planner",
                lease_role: "worker",
                purpose: "plan",
                capability: "repository_read",
            },
            Case {
                execution_role: "reviewer",
                task_type: "review",
                task_role: "reviewer",
                lease_role: "reviewer",
                purpose: "review",
                capability: "repository_read",
            },
            Case {
                execution_role: "validator",
                task_type: "validation",
                task_role: "validator",
                lease_role: "worker",
                purpose: "validate",
                capability: "repository_read",
            },
            Case {
                execution_role: "investigator",
                task_type: "discovery",
                task_role: "investigator",
                lease_role: "worker",
                purpose: "investigate",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "implementation",
                task_role: "implementer",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_write",
            },
            Case {
                execution_role: "interactive",
                task_type: "planning",
                task_role: "planner",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "review",
                task_role: "reviewer",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "validation",
                task_role: "validator",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "discovery",
                task_role: "investigator",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
        ];

        for case in cases {
            let task_role = if case.execution_role == "interactive" {
                crate::task_service::execution::task_role_for_task_type(case.task_type).to_owned()
            } else {
                db::canonical_task_role_name(case.execution_role)
                    .expect("explicit Execution role maps to a TaskRole")
            };
            let purpose = crate::task_service::execution::execution_purpose_for_task_type(
                case.task_type,
                case.execution_role,
            );
            assert_eq!(task_role, case.task_role);
            assert_eq!(
                canonical_workspace_lease_role(case.execution_role).expect("lease class"),
                case.lease_role
            );
            assert_eq!(purpose.to_string(), case.purpose);
            assert_eq!(
                workspace_lease_capability_class(
                    case.task_type,
                    Some(&purpose.to_string()),
                    case.execution_role,
                ),
                case.capability
            );
        }
    }
}
