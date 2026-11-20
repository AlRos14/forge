//! WorkUnit and WorkspaceLease admission using exact target authority.

use super::*;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use sha2::{Digest, Sha256};

const WORKSPACE_LEASE_SECONDS: i64 = 15 * 60;
const CAPABILITY_PROFILE_REVISION: &str = "forge.capability-profile/v1";

/// Current Tasks grant repository write capability to their assigned Agents.
fn workspace_lease_capability_class() -> &'static str {
    "repository_write"
}

impl TaskService {
    pub(crate) async fn prepare_work_unit_workspace_leases(
        &self,
        task: &db::Task,
        work_unit: &db::WorkUnit,
        execution: &db::CreateExecution,
        actor: &db::ActorRef,
    ) -> Result<Vec<CreateWorkspaceLease>> {
        let group = WorkspaceRepo::get_group_by_work_unit_id(&*self.db, &work_unit.id).await?;
        let workspaces =
            WorkUnitWorkspaceRepo::list_by_work_unit_id(&*self.db, &work_unit.id).await?;
        if let Some(group) = group {
            if group.task_id != task.id || group.work_unit_id != work_unit.id {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceGroup does not belong to the exact WorkUnit",
                ));
            }
            let formal_review = crate::role_capabilities::current_task_role_has_capability(
                &self.db,
                &db::TaskRoleRepo::get_by_task_and_role(
                    &*self.db,
                    &task.id,
                    &db::canonical_task_role_name(&work_unit.role).ok_or_else(|| {
                        ServiceError::invalid_operation("WorkUnit role is not a TaskRole")
                    })?,
                )
                .await?
                .ok_or_else(|| ServiceError::invalid_operation("WorkUnit TaskRole does not exist"))?
                .id,
                api_types::TaskRoleCapability::FormalReview,
            )
            .await?;
            let task_repositories = TaskRepo::list_repositories(&*self.db, &task.id).await?;
            let selected = WorkUnitRepo::list_repository_ids(&*self.db, &work_unit.id).await?;
            let (expected_repo_ids, writable_repo_ids) = if formal_review {
                if execution.workspace_id.is_some() {
                    return Err(ServiceError::invalid_operation(
                        "Review Execution must keep workspace_id NULL",
                    ));
                }
                let subject =
                    WorkUnitRepo::list_review_subject_repositories(&*self.db, &work_unit.id)
                        .await?;
                let repo_ids = subject
                    .iter()
                    .map(|repository| repository.repo_id.clone())
                    .collect::<HashSet<_>>();
                if repo_ids.is_empty()
                    || subject.iter().any(|repository| {
                        repository.result_execution_id.is_none()
                            || repository.result_commit_sha.is_none()
                            || repository.result_workspace_id.is_none()
                    })
                {
                    return Err(ServiceError::invalid_operation(
                        "Review WorkspaceGroup requires a fully frozen exact subject",
                    ));
                }
                (repo_ids, HashSet::new())
            } else {
                if !work_unit.requires_integration {
                    return Err(ServiceError::invalid_operation(
                        "non-mutating WorkUnit cannot receive a writable WorkspaceGroup",
                    ));
                }
                let mut writable = HashSet::new();
                let mut expected = HashSet::new();
                for repository in &task_repositories {
                    match repository.role {
                        db::TaskRepositoryRole::Context => {
                            expected.insert(repository.repo_id.clone());
                        }
                        db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                            if selected.is_empty() || selected.contains(&repository.repo_id) =>
                        {
                            expected.insert(repository.repo_id.clone());
                            writable.insert(repository.repo_id.clone());
                        }
                        db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected => {}
                    }
                }
                let primary_repo_id = task.repo_id.as_deref().unwrap_or_default();
                let primary_workspace = if writable.contains(primary_repo_id) {
                    workspaces
                        .iter()
                        .find(|workspace| workspace.repo_id == primary_repo_id)
                        .map(|workspace| workspace.id.as_str())
                } else {
                    None
                };
                if expected.is_empty() || execution.workspace_id.as_deref() != primary_workspace {
                    return Err(ServiceError::invalid_operation(
                        "Execution primary Workspace mirror does not match the exact writable scope",
                    ));
                }
                (expected, writable)
            };
            if expected_repo_ids.len() != workspaces.len() {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceGroup does not contain the exact repository set",
                ));
            }
            let mut leases = Vec::with_capacity(writable_repo_ids.len());
            for workspace in &workspaces {
                let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::not_found("workspace scope", workspace.id.clone())
                    })?;
                if !expected_repo_ids.contains(&workspace.repo_id)
                    || workspace.task_id != task.id
                    || workspace.status != WorkspaceStatus::Ready
                    || scope.kind != db::WorkspaceScopeKind::WorkUnit
                    || scope.task_id != task.id
                    || scope.work_unit_id.as_deref() != Some(work_unit.id.as_str())
                    || scope.workspace_group_id.as_deref() != Some(group.id.as_str())
                {
                    return Err(ServiceError::invalid_operation(
                        "WorkspaceGroup member is not ready in its exact durable scope",
                    ));
                }
                if writable_repo_ids.contains(&workspace.repo_id) {
                    leases.push(
                        self.prepare_work_unit_workspace_lease(
                            task, work_unit, workspace, execution, actor,
                        )
                        .await?,
                    );
                }
            }
            if workspaces
                .iter()
                .any(|workspace| !expected_repo_ids.contains(&workspace.repo_id))
            {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceGroup contains a repository outside the exact WorkUnit scope",
                ));
            }
            return Ok(leases);
        }

        if work_unit.requires_integration {
            let workspace_id = execution.workspace_id.as_deref().ok_or_else(|| {
                ServiceError::invalid_operation(
                    "repository WorkUnit Execution requires its exact Workspace",
                )
            })?;
            let workspace = workspaces
                .iter()
                .find(|workspace| workspace.id == workspace_id)
                .ok_or_else(|| ServiceError::not_found("workspace", workspace_id.to_owned()))?;
            let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, workspace_id)
                .await?
                .ok_or_else(|| ServiceError::not_found("workspace scope", workspace_id))?;
            if scope.kind != db::WorkspaceScopeKind::WorkUnit
                || scope.task_id != task.id
                || scope.work_unit_id.as_deref() != Some(work_unit.id.as_str())
                || workspace.repo_id != task.repo_id.as_deref().unwrap_or_default()
                || workspaces.len() != 1
                || workspace.status != WorkspaceStatus::Ready
            {
                return Err(ServiceError::invalid_operation(
                    "group-less historical Workspace is not the exact single-primary scope",
                ));
            }
            return Ok(vec![
                self.prepare_work_unit_workspace_lease(
                    task, work_unit, workspace, execution, actor,
                )
                .await?,
            ]);
        }
        if execution.workspace_id.is_some() || !workspaces.is_empty() {
            return Err(ServiceError::invalid_operation(
                "non-repository WorkUnit Execution cannot bind a repository Workspace",
            ));
        }
        Ok(Vec::new())
    }

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
        let repo_id = workspace.repo_id.as_str();
        if work_unit.task_id != task.id
            || work_unit.role != execution.role
            || execution.actor_ref.as_ref() != Some(actor)
            || !work_unit.requires_integration
        {
            return Err(ServiceError::invalid_operation(
                "WorkUnit, Execution, Actor, and Workspace lease bindings do not match",
            ));
        }
        self.ensure_task_runnable(task).await?;
        let canonical_role = self
            .workspace_lease_role_for_task_role(task, &work_unit.role)
            .await?;
        if canonical_role == "reviewer"
            && WorkspaceRepo::get_group_by_work_unit_id(&*self.db, &work_unit.id)
                .await?
                .is_some()
        {
            return Err(ServiceError::invalid_operation(
                "Formal Review WorkspaceGroups do not receive WorkspaceLeases",
            ));
        }
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
        let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
            .await?
            .filter(|scope| {
                scope.task_id == task.id
                    && scope.kind == db::WorkspaceScopeKind::WorkUnit
                    && scope.work_unit_id.as_deref() == Some(work_unit.id.as_str())
            })
            .ok_or_else(|| {
                ServiceError::invalid_operation(
                    "repository WorkspaceLease requires its exact WorkUnit Workspace scope",
                )
            })?;
        let group = WorkspaceRepo::get_group_by_work_unit_id(&*self.db, &work_unit.id).await?;
        if scope.workspace_group_id.as_deref() != group.as_ref().map(|group| group.id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "WorkspaceGroup identity does not match its exact Workspace scope",
            ));
        }
        let task_repositories = TaskRepo::list_repositories(&*self.db, &task.id).await?;
        let writable_repository = task_repositories.iter().any(|repository| {
            repository.repo_id == workspace.repo_id
                && matches!(
                    repository.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
        });
        if !writable_repository {
            return Err(ServiceError::invalid_operation(
                "WorkspaceLease repository is not writable in the exact Task scope",
            ));
        }
        let selected = WorkUnitRepo::list_repository_ids(&*self.db, &work_unit.id).await?;
        if !selected.is_empty() && !selected.contains(&workspace.repo_id) {
            return Err(ServiceError::invalid_operation(
                "WorkspaceLease repository is outside the exact WorkUnit allocation",
            ));
        }
        let workspaces =
            WorkUnitWorkspaceRepo::list_by_work_unit_id(&*self.db, &work_unit.id).await?;
        let primary_repo_id = task.repo_id.as_deref().unwrap_or_default();
        let primary_selected = task_repositories.iter().any(|repository| {
            repository.repo_id == primary_repo_id
                && matches!(
                    repository.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
                && (selected.is_empty() || selected.contains(&repository.repo_id))
        });
        let expected_primary_workspace = if primary_selected {
            workspaces
                .iter()
                .find(|candidate| candidate.repo_id == primary_repo_id)
                .map(|candidate| candidate.id.as_str())
        } else {
            None
        };
        if execution.workspace_id.as_deref() != expected_primary_workspace {
            return Err(ServiceError::invalid_operation(
                "Execution workspace_id must mirror the selected primary repository Workspace",
            ));
        }
        if workspace.status != WorkspaceStatus::Ready {
            return Err(ServiceError::invalid_operation(
                "WorkspaceLease requires a ready repository Workspace",
            ));
        }
        if group.is_none() && (workspace.repo_id != primary_repo_id || workspaces.len() != 1) {
            return Err(ServiceError::invalid_operation(
                "group-less historical Workspace authority is limited to its single primary repository",
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
            .workspace_lease_inputs(task, workspace, repo_id, &execution.id, true)
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

        let repo_id = workspace.repo_id.as_str();
        let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
            .await?
            .filter(|scope| {
                scope.task_id == task.id
                    && scope.kind == db::WorkspaceScopeKind::WorkUnit
                    && scope.work_unit_id.as_deref() == Some(work_unit_id)
            })
            .ok_or_else(|| ServiceError::invalid_operation("Workspace scope is not exact"))?;
        let group = WorkspaceRepo::get_group_by_work_unit_id(&*self.db, work_unit_id).await?;
        if scope.workspace_group_id.as_deref() != group.as_ref().map(|group| group.id.as_str()) {
            return Err(ServiceError::invalid_operation(
                "WorkspaceGroup identity does not match its exact Workspace scope",
            ));
        }
        let task_repositories = TaskRepo::list_repositories(&*self.db, &task.id).await?;
        let writable_repository = task_repositories.iter().any(|repository| {
            repository.repo_id == workspace.repo_id
                && matches!(
                    repository.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
        });
        let selected = WorkUnitRepo::list_repository_ids(&*self.db, work_unit_id).await?;
        if !writable_repository || (!selected.is_empty() && !selected.contains(&workspace.repo_id))
        {
            return Err(ServiceError::invalid_operation(
                "WorkspaceLease repository is outside the exact writable WorkUnit scope",
            ));
        }
        let workspaces =
            WorkUnitWorkspaceRepo::list_by_work_unit_id(&*self.db, work_unit_id).await?;
        let primary_repo_id = task.repo_id.as_deref().unwrap_or_default();
        let primary_selected = task_repositories.iter().any(|repository| {
            repository.repo_id == primary_repo_id
                && matches!(
                    repository.role,
                    db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                )
                && (selected.is_empty() || selected.contains(&repository.repo_id))
        });
        let expected_primary_workspace = if primary_selected {
            workspaces
                .iter()
                .find(|candidate| candidate.repo_id == primary_repo_id)
                .map(|candidate| candidate.id.as_str())
        } else {
            None
        };
        if execution.workspace_id.as_deref() != expected_primary_workspace
            || (group.is_none() && (workspace.repo_id != primary_repo_id || workspaces.len() != 1))
        {
            return Err(ServiceError::invalid_operation(
                "Execution Workspace mirror does not match the exact WorkUnit scope",
            ));
        }
        let (repo, capability_class, base_ref) = self
            .workspace_lease_inputs(task, workspace, repo_id, execution_id, false)
            .await?;
        if repo.project_id != task.project_id {
            return Err(ServiceError::invalid_operation(
                "WorkUnit WorkspaceLease repository belongs to another Project",
            ));
        }
        let lease = WorkspaceLeaseRepo::get_active_for_execution_workspace(
            &*self.db,
            execution_id,
            &workspace.id,
        )
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
        let canonical_role = self
            .workspace_lease_role_for_task_role(task, &unit.role)
            .await?;
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
        is_new_execution: bool,
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
        if !is_new_execution {
            let _execution = ExecutionRepo::get_by_id(&*self.db, execution_id)
                .await?
                .filter(|execution| execution.task_id == task.id)
                .ok_or_else(|| {
                    ServiceError::invalid_operation(
                        "WorkspaceLease requires an exact Task Execution",
                    )
                })?;
        }
        let capability_class = workspace_lease_capability_class().to_owned();
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
        match WorkspaceLeaseRepo::list_active_for_work_unit(&*self.db, work_unit_id).await {
            Ok(leases) => {
                for lease in leases {
                    if lease.execution_id == execution_id {
                        self.revoke_workspace_lease(&lease).await;
                    }
                }
            }
            Err(error) => {
                tracing::warn!(%error, task_id, "failed to load terminal WorkspaceLeases")
            }
        }
    }

    pub(crate) async fn verify_execution_workspace_authority(
        &self,
        execution: &db::Execution,
    ) -> Result<Option<db::WorkspaceLease>> {
        if crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::Orchestration,
        )
        .await?
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
        let formal_review = crate::role_capabilities::execution_has_capability(
            &self.db,
            &execution.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?;
        if let Some(group) =
            WorkspaceRepo::get_group_by_work_unit_id(&*self.db, work_unit_id).await?
        {
            if group.task_id != task.id || group.work_unit_id != unit.id {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceGroup does not belong to the exact WorkUnit",
                ));
            }
            let workspaces =
                WorkUnitWorkspaceRepo::list_by_work_unit_id(&*self.db, work_unit_id).await?;
            if workspaces.is_empty() {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceGroup has no repository workspaces",
                ));
            }
            let task_repositories = TaskRepo::list_repositories(&*self.db, &task.id).await?;
            let selected = WorkUnitRepo::list_repository_ids(&*self.db, work_unit_id).await?;
            let expected_repositories = if formal_review {
                let subject =
                    WorkUnitRepo::list_review_subject_repositories(&*self.db, work_unit_id).await?;
                if subject.is_empty() {
                    return Err(ServiceError::invalid_operation(
                        "Review WorkspaceGroup has no exact subject repositories",
                    ));
                }
                subject
                    .into_iter()
                    .map(|repository| repository.repo_id)
                    .collect::<HashSet<_>>()
            } else {
                if !unit.requires_integration {
                    return Err(ServiceError::invalid_operation(
                        "non-mutating WorkUnit cannot use a writable WorkspaceGroup",
                    ));
                }
                task_repositories
                    .iter()
                    .filter(|repository| {
                        repository.role == db::TaskRepositoryRole::Context
                            || (matches!(
                                repository.role,
                                db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                            ) && (selected.is_empty()
                                || selected.contains(&repository.repo_id)))
                    })
                    .map(|repository| repository.repo_id.clone())
                    .collect::<HashSet<_>>()
            };
            if expected_repositories.is_empty() || workspaces.len() != expected_repositories.len() {
                return Err(ServiceError::invalid_operation(
                    "WorkspaceGroup does not match the exact Task and WorkUnit repository scope",
                ));
            }
            let primary_repo_id = task.repo_id.as_deref().unwrap_or_default();
            let primary_selected = !formal_review
                && expected_repositories.contains(primary_repo_id)
                && task_repositories.iter().any(|repository| {
                    repository.repo_id == primary_repo_id
                        && matches!(
                            repository.role,
                            db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                        )
                });
            let primary_workspace = if primary_selected {
                workspaces
                    .iter()
                    .find(|workspace| workspace.repo_id == primary_repo_id)
                    .map(|workspace| workspace.id.as_str())
            } else {
                None
            };
            if (formal_review && execution.workspace_id.is_some())
                || (!formal_review && execution.workspace_id.as_deref() != primary_workspace)
            {
                return Err(ServiceError::invalid_operation(
                    "Execution workspace_id is not the exact primary repository mirror",
                ));
            }
            let mut writable_ids = HashSet::new();
            for workspace in &workspaces {
                let scope = WorkUnitWorkspaceRepo::get_scope_by_id(&*self.db, &workspace.id)
                    .await?
                    .ok_or_else(|| {
                        ServiceError::not_found("workspace scope", workspace.id.clone())
                    })?;
                if !expected_repositories.contains(&workspace.repo_id)
                    || workspace.task_id != task.id
                    || workspace.status != WorkspaceStatus::Ready
                    || scope.task_id != task.id
                    || scope.kind != db::WorkspaceScopeKind::WorkUnit
                    || scope.work_unit_id.as_deref() != Some(work_unit_id)
                    || scope.workspace_group_id.as_deref() != Some(group.id.as_str())
                {
                    return Err(ServiceError::invalid_operation(
                        "WorkspaceGroup contains a non-ready or mismatched repository scope",
                    ));
                }
                let is_writable = !formal_review
                    && task_repositories.iter().any(|repository| {
                        repository.repo_id == workspace.repo_id
                            && matches!(
                                repository.role,
                                db::TaskRepositoryRole::Primary | db::TaskRepositoryRole::Affected
                            )
                            && (selected.is_empty() || selected.contains(&repository.repo_id))
                    });
                if is_writable {
                    writable_ids.insert(workspace.id.clone());
                    self.verify_active_workspace_lease(
                        &task,
                        workspace,
                        &execution.role,
                        execution.agent_id.as_deref(),
                        &execution.id,
                    )
                    .await?;
                } else {
                    if !workspace.branch.starts_with("detached:")
                        || workspace
                            .before_sha
                            .as_deref()
                            .is_none_or(|sha| workspace.branch != format!("detached:{sha}"))
                    {
                        return Err(ServiceError::invalid_operation(
                            "read-only WorkspaceGroup member is not pinned to its exact commit",
                        ));
                    }
                    if WorkspaceLeaseRepo::get_active_for_execution_workspace(
                        &*self.db,
                        &execution.id,
                        &workspace.id,
                    )
                    .await?
                    .is_some()
                    {
                        return Err(ServiceError::invalid_operation(
                            "read-only WorkspaceGroup member cannot receive a WorkspaceLease",
                        ));
                    }
                }
            }
            let active_leases =
                WorkspaceLeaseRepo::list_active_for_work_unit(&*self.db, work_unit_id).await?;
            if active_leases.iter().any(|lease| {
                lease.execution_id == execution.id
                    && lease
                        .workspace_id
                        .as_ref()
                        .is_none_or(|workspace_id| !writable_ids.contains(workspace_id))
            }) || (formal_review
                && active_leases
                    .iter()
                    .any(|lease| lease.execution_id == execution.id))
            {
                return Err(ServiceError::invalid_operation(
                    "Execution has WorkspaceLease authority outside its exact writable repository set",
                ));
            }
            return Ok(active_leases
                .into_iter()
                .find(|lease| lease.execution_id == execution.id));
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

    async fn workspace_lease_role_for_task_role(
        &self,
        task: &db::Task,
        role_name: &str,
    ) -> Result<&'static str> {
        let canonical = db::canonical_task_role_name(role_name)
            .ok_or_else(|| ServiceError::invalid_operation("WorkUnit role is not a TaskRole"))?;
        let role = db::TaskRoleRepo::get_by_task_and_role(&*self.db, &task.id, &canonical)
            .await?
            .ok_or_else(|| ServiceError::invalid_operation("WorkUnit TaskRole does not exist"))?;
        let is_formal_review = crate::role_capabilities::current_task_role_has_capability(
            &self.db,
            &role.id,
            api_types::TaskRoleCapability::FormalReview,
        )
        .await?;
        Ok(canonical_workspace_lease_role(is_formal_review))
    }
}

fn canonical_workspace_lease_role(is_formal_review: bool) -> &'static str {
    if is_formal_review {
        "reviewer"
    } else {
        "worker"
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
    matches!(capability_class, "repository_write")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_lease_roles_preserve_reviewer_and_bound_custom_workers() {
        assert_eq!(canonical_workspace_lease_role(true), "reviewer");
        assert_eq!(canonical_workspace_lease_role(false), "worker");
        assert_eq!(canonical_workspace_lease_role(false), "worker");
    }
}
