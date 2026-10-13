//! WorkUnit and WorkspaceLease admission using exact target authority.

use super::*;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};

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
            .filter(|scope| {
                scope.task_id == task.id
                    && scope.kind == db::WorkspaceScopeKind::WorkUnit
                    && scope.work_unit_id.is_some()
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "Task-scoped Integration Workspace cannot authorize an Execution",
                )
            })?;
        self.verify_active_work_unit_workspace_lease(
            task,
            workspace,
            scope.work_unit_id.as_deref().expect("scope checked above"),
            role,
            principal_id,
            execution_id,
        )
        .await
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
            || unit.assigned_actor.as_ref() != Some(&db::ActorRef::Agent(agent_id.clone()))
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
        let Some(work_unit_id) = execution.work_unit_id.as_deref() else {
            return;
        };
        let active_lease =
            WorkspaceLeaseRepo::get_active_for_work_unit(&*self.db, work_unit_id).await;
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
        let work_unit_id = execution.work_unit_id.as_deref().ok_or_else(|| {
            ServiceError::invalid_operation("Task-scoped Execution has no exact WorkUnit authority")
        })?;
        let unit = WorkUnitRepo::get_by_id(&*self.db, work_unit_id)
            .await?
            .filter(|unit| unit.task_id == task.id)
            .ok_or_else(|| ServiceError::not_found("work_unit", work_unit_id.to_owned()))?;
        if unit.status != db::WorkUnitStatus::Open
            || execution.work_unit_version != Some(unit.version)
            || execution.role != unit.role
            || unit.assigned_actor.as_ref() != execution.actor_ref().as_ref()
        {
            return Err(ServiceError::invalid_operation(
                "Execution no longer matches its exact WorkUnit revision and Actor assignment",
            ));
        }
        if !unit.requires_integration {
            if execution.workspace_id.is_some() {
                return Err(ServiceError::invalid_operation(
                    "non-repository WorkUnit Execution cannot use a Workspace",
                ));
            }
            return Ok(None);
        }
        let Some(workspace_id) = execution.workspace_id.as_deref() else {
            return Err(ServiceError::invalid_operation(
                "repository WorkUnit Execution requires its exact Workspace",
            ));
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
            lease_role: &'static str,
            purpose: &'static str,
            capability: &'static str,
        }
        let cases = [
            Case {
                execution_role: "implementer",
                task_type: "implementation",
                lease_role: "worker",
                purpose: "implement",
                capability: "repository_write",
            },
            Case {
                execution_role: "planner",
                task_type: "planning",
                lease_role: "worker",
                purpose: "plan",
                capability: "repository_read",
            },
            Case {
                execution_role: "reviewer",
                task_type: "review",
                lease_role: "reviewer",
                purpose: "review",
                capability: "repository_read",
            },
            Case {
                execution_role: "validator",
                task_type: "validation",
                lease_role: "worker",
                purpose: "validate",
                capability: "repository_read",
            },
            Case {
                execution_role: "investigator",
                task_type: "discovery",
                lease_role: "worker",
                purpose: "investigate",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "implementation",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_write",
            },
            Case {
                execution_role: "interactive",
                task_type: "planning",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "review",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "validation",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
            Case {
                execution_role: "interactive",
                task_type: "discovery",
                lease_role: "worker",
                purpose: "general",
                capability: "repository_read",
            },
        ];

        for case in cases {
            let purpose = crate::task_service::execution::execution_purpose_for_task_type(
                case.task_type,
                case.execution_role,
            );
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
