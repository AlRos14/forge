use super::*;
use crate::{new_uuid_v4, now_rfc3339};

#[async_trait]
impl ProjectRepo for SqliteDb {
    async fn create(&self, input: CreateProject) -> Result<Project> {
        let member_id = input.owner_id.as_ref().map(|_| new_uuid_v4());
        self.create_project_record(input, member_id.as_deref())
            .await
    }

    async fn create_with_owner_membership(
        &self,
        input: CreateProject,
        member_id: &str,
    ) -> Result<Project> {
        self.create_project_record(input, Some(member_id)).await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Project>> {
        sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM project WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_project)
        .transpose()
    }

    async fn get_by_id_in_tx(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
    ) -> Result<Option<Project>> {
        sqlx::query(&format!(
            "SELECT {PROJECT_COLUMNS} FROM project WHERE id = ?"
        ))
        .bind(id)
        .fetch_optional(&mut **transaction)
        .await?
        .map(map_project)
        .transpose()
    }

    async fn set_owner_in_tx(
        &self,
        transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
        id: &str,
        owner_id: &str,
        updated_at: &str,
    ) -> Result<()> {
        let result = sqlx::query(
            "UPDATE project SET owner_id = ?, updated_at = ?, version = version + 1 WHERE id = ?",
        )
        .bind(owner_id)
        .bind(updated_at)
        .bind(id)
        .execute(&mut **transaction)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn list(&self, page: PageRequest) -> Result<Page<Project>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT {PROJECT_COLUMNS} FROM project ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_project)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            total_count(&self.pool, "SELECT COUNT(*) FROM project").await?
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn list_visible_to_user(
        &self,
        user_id: &str,
        page: PageRequest,
    ) -> Result<Page<Project>> {
        let offset = decode_offset(&page.cursor)?;
        let visibility_filter = "owner_id IS NULL OR EXISTS (
            SELECT 1 FROM project_member
            WHERE project_member.project_id = project.id AND project_member.user_id = ?
        )";
        let sql = format!(
            "SELECT {PROJECT_COLUMNS} FROM project WHERE ({visibility_filter}) ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(user_id)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_project)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            Some(
                sqlx::query_scalar::<_, i64>(&format!(
                    "SELECT COUNT(*) FROM project WHERE ({visibility_filter})"
                ))
                .bind(user_id)
                .fetch_one(&self.pool)
                .await?,
            )
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn update(&self, input: UpdateProject) -> Result<Project> {
        let mut project = ProjectRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if let Some(name) = input.name {
            project.name = name;
        }
        if let Some(settings) = input.settings {
            project.settings = settings;
        }
        if let Some(primary_repo_id) = input.primary_repo_id {
            project.primary_repo_id = primary_repo_id;
        }
        if let Some(paused_at) = input.paused_at {
            project.paused_at = paused_at;
        }
        project.updated_at = input.updated_at;
        sqlx::query(
            "UPDATE project SET name = ?, settings = ?, primary_repo_id = ?, paused_at = ?, project_hooks_json = ?, project_work_epoch = ?, updated_at = ? WHERE id = ?",
        )
        .bind(&project.name)
        .bind(&project.settings)
        .bind(project.primary_repo_id.as_deref())
        .bind(project.paused_at.as_deref())
        .bind(&project.project_hooks_json)
        .bind(project.project_work_epoch)
        .bind(&project.updated_at)
        .bind(&project.id)
        .execute(&self.pool)
        .await?;
        Ok(project)
    }

    async fn update_at_version(
        &self,
        input: UpdateProject,
        expected_version: i64,
        project_hooks_json: Option<String>,
        caller_user_id: Option<&str>,
    ) -> Result<Project> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(caller_user_id) = caller_user_id {
            let is_admin: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_member
                    WHERE project_id = ? AND user_id = ? AND role IN ('owner', 'admin')
                )",
            )
            .bind(&input.id)
            .bind(caller_user_id)
            .fetch_one(&mut *tx)
            .await?;
            if is_admin == 0 {
                return Err(DbError::ProjectAdminRequired);
            }
        }
        let mut project = ProjectRepo::get_by_id_in_tx(self, &mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        if project.version != expected_version {
            return Err(DbError::VersionConflict);
        }
        if let Some(name) = input.name {
            project.name = name;
        }
        if let Some(settings) = input.settings {
            project.settings = settings;
        }
        if let Some(primary_repo_id) = input.primary_repo_id {
            project.primary_repo_id = primary_repo_id;
        }
        if let Some(paused_at) = input.paused_at {
            project.paused_at = paused_at;
        }
        if let Some(project_hooks_json) = project_hooks_json {
            project.project_hooks_json = project_hooks_json;
        }
        project.updated_at = input.updated_at;
        let result = sqlx::query(
            "UPDATE project
             SET name = ?, settings = ?, primary_repo_id = ?, paused_at = ?,
                 project_hooks_json = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ?",
        )
        .bind(&project.name)
        .bind(&project.settings)
        .bind(project.primary_repo_id.as_deref())
        .bind(project.paused_at.as_deref())
        .bind(&project.project_hooks_json)
        .bind(&project.updated_at)
        .bind(&project.id)
        .bind(expected_version)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        tx.commit().await?;
        ProjectRepo::get_by_id(self, &project.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn set_project_hooks_json(
        &self,
        id: &str,
        project_hooks_json: &str,
        updated_at: &str,
    ) -> Result<()> {
        let result =
            sqlx::query("UPDATE project SET project_hooks_json = ?, updated_at = ? WHERE id = ?")
                .bind(project_hooks_json)
                .bind(updated_at)
                .bind(id)
                .execute(&self.pool)
                .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        Ok(())
    }

    async fn increment_project_work_epoch(
        &self,
        tx: &mut Transaction<'_, Sqlite>,
        project_id: &str,
        by: i64,
    ) -> Result<i64> {
        let epoch = sqlx::query_scalar::<_, i64>(
            "UPDATE project SET project_work_epoch = project_work_epoch + ? WHERE id = ? RETURNING project_work_epoch",
        )
        .bind(by)
        .bind(project_id)
        .fetch_optional(&mut **tx)
        .await?;
        epoch.ok_or(DbError::NotFound)
    }

    async fn set_paused_at(
        &self,
        id: &str,
        paused_at: Option<String>,
        caller_user_id: Option<&str>,
    ) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(caller_user_id) = caller_user_id {
            let is_admin: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_member
                    WHERE project_id = ? AND user_id = ? AND role IN ('owner', 'admin')
                )",
            )
            .bind(id)
            .bind(caller_user_id)
            .fetch_one(&mut *tx)
            .await?;
            if is_admin == 0 {
                return Err(DbError::ProjectAdminRequired);
            }
        }
        let result = sqlx::query("UPDATE project SET paused_at = ?, updated_at = ? WHERE id = ?")
            .bind(paused_at.as_deref())
            .bind(now_rfc3339())
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    async fn delete(&self, id: &str, caller_user_id: Option<&str>) -> Result<()> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(caller_user_id) = caller_user_id {
            let is_admin: i64 = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM project_member
                    WHERE project_id = ? AND user_id = ? AND role IN ('owner', 'admin')
                )",
            )
            .bind(id)
            .bind(caller_user_id)
            .fetch_one(&mut *tx)
            .await?;
            if is_admin == 0 {
                return Err(DbError::ProjectAdminRequired);
            }
        }
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM project WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?
            .is_some();
        if !exists {
            return Err(DbError::NotFound);
        }

        let has_immutable_releases = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(SELECT 1 FROM project_release WHERE project_id = ?)",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if has_immutable_releases != 0 {
            return Err(DbError::ProjectHasReleases);
        }

        let active_integration_operation = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS(
                 SELECT 1 FROM task_integration_operation op
                 JOIN task t ON t.id = op.task_id
                 WHERE t.project_id = ? AND op.status = 'running'
             )",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await?;
        if active_integration_operation != 0 {
            return Err(DbError::Check(
                "Project deletion is blocked by an active Task integration operation".to_owned(),
            ));
        }

        // Immutable orchestration rows remain protected from individual
        // deletion. Project deletion is the one bounded teardown operation:
        // the transaction installs a Project-scoped guard, removes immutable
        // leaves in dependency order, then lets the existing cascades remove
        // mutable projections. Deferring FKs closes self-referential and
        // cross-artifact RESTRICT edges until the whole Project is gone.
        sqlx::query("PRAGMA defer_foreign_keys = ON")
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO project_deletion_guard (project_id, created_at) VALUES (?, ?)")
            .bind(id)
            .bind(now_rfc3339())
            .execute(&mut *tx)
            .await?;

        for statement in [
            "DELETE FROM execution_follow_up_receipt WHERE parent_execution_id IN
                 (SELECT e.id FROM execution e JOIN task t ON t.id = e.task_id WHERE t.project_id = ?)",
            "DELETE FROM orchestrator_action WHERE execution_id IN
                 (SELECT e.id FROM execution e JOIN task t ON t.id = e.task_id WHERE t.project_id = ?)",
            "DELETE FROM orchestrator_wake_execution WHERE wake_id IN
                 (SELECT id FROM orchestrator_wake WHERE task_id IN
                   (SELECT id FROM task WHERE project_id = ?))",
            "DELETE FROM orchestrator_wake WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM remote_pr_history WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM remote_pr_admission WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM legacy_pr_history WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM pr_metadata WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM pr_provider_config WHERE repo_id IN
                 (SELECT id FROM repo WHERE project_id = ?)",
            "DELETE FROM task_retry_override WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM task_failure_retry_receipt WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM task_integration_operation WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM task_lifecycle_transition WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM gate_evaluation_input WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM gate_evaluation WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM gate_policy_revision WHERE gate_id IN
                 (SELECT g.id FROM gate g JOIN task t ON t.id = g.task_id
                  WHERE t.project_id = ?)",
            "DELETE FROM gate WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM task_lifecycle_migration_audit WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM task_lifecycle WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM message_work_unit WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM handoff_work_unit WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM work_unit_integration WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM work_unit_dependency WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM workspace_scope WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM workspace_lease WHERE project_id = ?",
            "DELETE FROM execution_artifact_input WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM review_execution_subject WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM execution_artifact_output WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "UPDATE execution SET work_unit_id = NULL, work_unit_version = NULL
             WHERE work_unit_id IS NOT NULL AND task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM work_unit WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM message_artifact WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM handoff_artifact WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM proposal_artifact WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM decision_actor WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM artifact_execution_producer WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM decision WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM message WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM handoff WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM proposal WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM artifact WHERE task_id IN
                 (SELECT id FROM task WHERE project_id = ?)",
            "DELETE FROM media_asset_tombstone WHERE asset_id IN
                 (SELECT id FROM media_asset WHERE project_id = ?)",
        ] {
            sqlx::query(statement).bind(id).execute(&mut *tx).await?;
        }

        let result = sqlx::query("DELETE FROM project WHERE id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        sqlx::query("DELETE FROM project_deletion_guard WHERE project_id = ?")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}

impl SqliteDb {
    async fn create_project_record(
        &self,
        input: CreateProject,
        owner_member_id: Option<&str>,
    ) -> Result<Project> {
        if owner_member_id.is_some() && input.owner_id.is_none() {
            return Err(DbError::Check(
                "owner membership requires a canonical Project owner".to_owned(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        sqlx::query("INSERT INTO project (id, name, settings, primary_repo_id, owner_id, project_hooks_json, project_work_epoch, created_at, updated_at) VALUES (?, ?, ?, ?, ?, '[]', 0, ?, ?)")
            .bind(&input.id)
            .bind(&input.name)
            .bind(&input.settings)
            .bind(&input.primary_repo_id)
            .bind(&input.owner_id)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&mut *transaction)
            .await?;

        if let (Some(member_id), Some(owner_id)) = (owner_member_id, input.owner_id.as_deref()) {
            sqlx::query(
                "INSERT INTO project_member (id, project_id, user_id, role, created_at, updated_at) \
                 VALUES (?, ?, ?, 'owner', ?, ?)",
            )
            .bind(member_id)
            .bind(&input.id)
            .bind(owner_id)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&mut *transaction)
            .await?;
        }

        let correlation_id = new_uuid_v4();
        let actor_type = if input.owner_id.is_some() {
            "user"
        } else {
            "system"
        }
        .to_owned();
        let actor_id = input.owner_id.clone();
        let event = CreateDomainEvent {
            id: new_uuid_v4(),
            event_type: "project.created".to_owned(),
            entity_type: "project".to_owned(),
            entity_id: input.id.clone(),
            actor_type,
            actor_id,
            scope_type: "project".to_owned(),
            scope_id: input.id.clone(),
            correlation_id,
            causation_id: None,
            causation_depth: 0,
            dedupe_key: Some(format!("project-created:{}", input.id)),
            payload_json: serde_json::json!({
                "project_id": input.id,
                "name": input.name,
            })
            .to_string(),
            created_at: input.created_at.clone(),
        };
        DomainEventRepo::append_event_in_tx(self, &mut transaction, &event).await?;
        transaction.commit().await?;
        ProjectRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }
}
