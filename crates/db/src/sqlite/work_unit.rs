use super::*;

fn actor_from_columns(kind: Option<String>, id: Option<String>) -> Result<Option<ActorRef>> {
    match (kind, id) {
        (None, None) => Ok(None),
        (Some(kind), Some(id)) => {
            let kind: ActorKind = parse_enum(kind)?;
            Ok(Some(match kind {
                ActorKind::Human => ActorRef::Human(id),
                ActorKind::Agent => ActorRef::Agent(id),
            }))
        }
        _ => Err(DbError::Check(
            "WorkUnit ActorRef columns are incomplete".to_owned(),
        )),
    }
}

fn actor_columns(actor: Option<&ActorRef>) -> (Option<String>, Option<String>) {
    actor
        .map(|actor| (Some(actor.kind().to_string()), Some(actor.id().to_owned())))
        .unwrap_or((None, None))
}

fn provenance_from_columns(
    kind: Option<String>,
    id: Option<String>,
    actor_kind: Option<String>,
) -> Result<Option<WorkUnitProvenance>> {
    match (kind, id, actor_kind) {
        (None, None, None) => Ok(None),
        (Some(kind), Some(id), actor_kind) => {
            let provenance = match kind.as_str() {
                "actor" => match actor_kind {
                    Some(actor_kind) => {
                        let actor_kind: ActorKind = parse_enum(actor_kind)?;
                        WorkUnitProvenance::Actor(match actor_kind {
                            ActorKind::Human => ActorRef::Human(id),
                            ActorKind::Agent => ActorRef::Agent(id),
                        })
                    }
                    None => WorkUnitProvenance::LegacyActor(id),
                },
                "work_unit" if actor_kind.is_none() => WorkUnitProvenance::WorkUnit(id),
                "artifact" if actor_kind.is_none() => WorkUnitProvenance::Artifact(id),
                "external" if actor_kind.is_none() => WorkUnitProvenance::External(id),
                _ => {
                    return Err(DbError::Check(
                        "WorkUnit provenance columns are inconsistent".to_owned(),
                    ));
                }
            };
            Ok(Some(provenance))
        }
        _ => Err(DbError::Check(
            "WorkUnit provenance columns are incomplete".to_owned(),
        )),
    }
}

fn provenance_columns(
    provenance: Option<&WorkUnitProvenance>,
) -> (Option<String>, Option<String>, Option<String>) {
    match provenance {
        None => (None, None, None),
        Some(WorkUnitProvenance::Actor(actor)) => (
            Some("actor".to_owned()),
            Some(actor.id().to_owned()),
            Some(actor.kind().to_string()),
        ),
        Some(WorkUnitProvenance::WorkUnit(id)) => {
            (Some("work_unit".to_owned()), Some(id.clone()), None)
        }
        Some(WorkUnitProvenance::Artifact(id)) => {
            (Some("artifact".to_owned()), Some(id.clone()), None)
        }
        Some(WorkUnitProvenance::External(id)) => {
            (Some("external".to_owned()), Some(id.clone()), None)
        }
        Some(WorkUnitProvenance::LegacyActor(id)) => {
            (Some("actor".to_owned()), Some(id.clone()), None)
        }
    }
}

fn map_work_unit(row: SqliteRow) -> Result<WorkUnit> {
    Ok(WorkUnit {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        parent_work_unit_id: row.try_get("parent_work_unit_id")?,
        title: row.try_get("title")?,
        scope: row.try_get("scope")?,
        status: parse_enum(row.try_get::<String, _>("status")?)?,
        role: row.try_get("role")?,
        assigned_actor: actor_from_columns(
            row.try_get("assigned_actor_kind")?,
            row.try_get("assigned_actor_id")?,
        )?,
        requires_integration: row.try_get::<i64, _>("requires_integration")? != 0,
        provenance: provenance_from_columns(
            row.try_get("provenance_kind")?,
            row.try_get("provenance_id")?,
            row.try_get("provenance_actor_kind")?,
        )?,
        created_by: actor_from_columns(
            Some(row.try_get("created_by_kind")?),
            Some(row.try_get("created_by_id")?),
        )?
        .ok_or_else(|| DbError::Check("WorkUnit creator ActorRef is missing".to_owned()))?,
        version: row.try_get("version")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_dependency(row: SqliteRow) -> Result<WorkUnitDependency> {
    let created_by = actor_from_columns(
        Some(row.try_get("created_by_kind")?),
        Some(row.try_get("created_by_id")?),
    )?
    .ok_or_else(|| DbError::Check("WorkUnit dependency creator is missing".to_owned()))?;
    Ok(WorkUnitDependency {
        task_id: row.try_get("task_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        depends_on_work_unit_id: row.try_get("depends_on_work_unit_id")?,
        created_by,
        created_at: row.try_get("created_at")?,
        satisfied: row.try_get::<i64, _>("satisfied")? != 0,
    })
}

fn map_integration(row: SqliteRow) -> Result<WorkUnitIntegration> {
    Ok(WorkUnitIntegration {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        execution_id: row.try_get("execution_id")?,
        source_workspace_id: row.try_get("source_workspace_id")?,
        source_branch: row.try_get("source_branch")?,
        source_sha: row.try_get("source_sha")?,
        target_workspace_id: row.try_get("target_workspace_id")?,
        target_branch: row.try_get("target_branch")?,
        target_before_sha: row.try_get("target_before_sha")?,
        target_after_sha: row.try_get("target_after_sha")?,
        operation_idempotency_key: row.try_get("operation_idempotency_key")?,
        outcome: parse_enum(row.try_get::<String, _>("outcome")?)?,
        conflict_metadata_json: row.try_get("conflict_metadata_json")?,
        version: row.try_get("version")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

async fn append_event(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    event: &CreateDomainEvent,
) -> Result<DomainEvent> {
    DomainEventRepo::append_event_in_tx(db, tx, event).await
}

fn work_unit_write_error(error: sqlx::Error) -> DbError {
    if let sqlx::Error::Database(database_error) = &error {
        let message = database_error.message().to_ascii_lowercase();
        if message.contains("dependency would create a cycle") {
            return DbError::CycleDetected;
        }
        if message.contains("agent reached concurrent execution capacity") {
            return DbError::AgentAtCapacity;
        }
        if message.contains("unique constraint failed: work_unit_dependency") {
            return DbError::IdempotencyConflict;
        }
        if message.contains("version") || message.contains("immutable") {
            return DbError::VersionConflict;
        }
        if message.contains("constraint failed") || message.contains("invalid") {
            return DbError::Check(database_error.message().to_owned());
        }
    }
    check_error(error)
}

#[async_trait]
impl WorkUnitRepo for SqliteDb {
    async fn create(
        &self,
        input: CreateWorkUnit,
        event: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>> {
        let (assigned_kind, assigned_id) = actor_columns(input.assigned_actor.as_ref());
        let (provenance_kind, provenance_id, provenance_actor_kind) =
            provenance_columns(input.provenance.as_ref());
        let created_kind = input.created_by.kind().to_string();
        let created_id = input.created_by.id().to_owned();
        let now = input.created_at.clone();
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO work_unit (
                id, task_id, parent_work_unit_id, title, scope, status, role,
                assigned_actor_kind, assigned_actor_id, requires_integration,
                provenance_kind, provenance_id, provenance_actor_kind,
                created_by_kind, created_by_id,
                version, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, 'open', ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(input.parent_work_unit_id.as_deref())
        .bind(&input.title)
        .bind(&input.scope)
        .bind(&input.role)
        .bind(assigned_kind.as_deref())
        .bind(assigned_id.as_deref())
        .bind(i64::from(input.requires_integration))
        .bind(provenance_kind.as_deref())
        .bind(provenance_id.as_deref())
        .bind(provenance_actor_kind.as_deref())
        .bind(&created_kind)
        .bind(&created_id)
        .bind(&now)
        .bind(&now)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        let event = append_event(self, &mut tx, &event).await?;
        let row = sqlx::query("SELECT * FROM work_unit WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await?;
        let record = map_work_unit(row)?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn get_task_id(&self, id: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT task_id FROM work_unit WHERE id = ?")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<WorkUnit>> {
        sqlx::query("SELECT * FROM work_unit WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_work_unit)
            .transpose()
    }

    async fn list_by_task(&self, task_id: &str) -> Result<Vec<WorkUnit>> {
        sqlx::query("SELECT * FROM work_unit WHERE task_id = ? ORDER BY created_at, id")
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_work_unit)
            .collect()
    }

    async fn update(
        &self,
        input: UpdateWorkUnit,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>> {
        let mut tx = self.pool.begin().await?;
        let current = sqlx::query("SELECT * FROM work_unit WHERE id = ?")
            .bind(&input.id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        let current = map_work_unit(current)?;
        if current.version != input.expected_version {
            return Err(DbError::VersionConflict);
        }
        if current.status != WorkUnitStatus::Open {
            return Err(DbError::InvalidTransition);
        }
        let title = input.title.as_deref().unwrap_or(&current.title);
        let scope = input.scope.as_deref().unwrap_or(&current.scope);
        let parent_id = input
            .parent_work_unit_id
            .as_ref()
            .unwrap_or(&current.parent_work_unit_id);
        let requires_integration = input
            .requires_integration
            .unwrap_or(current.requires_integration);
        let result = sqlx::query(
            "UPDATE work_unit SET title = ?, scope = ?, parent_work_unit_id = ?,
                 requires_integration = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND status = 'open'",
        )
        .bind(title)
        .bind(scope)
        .bind(parent_id.as_deref())
        .bind(i64::from(requires_integration))
        .bind(&input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_work_unit(
            sqlx::query("SELECT * FROM work_unit WHERE id = ?")
                .bind(&input.id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn allocate(
        &self,
        input: AllocateWorkUnit,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>> {
        let (assigned_kind, assigned_id) = actor_columns(input.assigned_actor.as_ref());
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE work_unit SET role = ?, assigned_actor_kind = ?, assigned_actor_id = ?,
                 version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND status = 'open'",
        )
        .bind(&input.role)
        .bind(assigned_kind.as_deref())
        .bind(assigned_id.as_deref())
        .bind(&input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        if result.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_work_unit(
            sqlx::query("SELECT * FROM work_unit WHERE id = ?")
                .bind(&input.id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn transition(
        &self,
        input: TransitionWorkUnit,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>> {
        if input.status == WorkUnitStatus::Open {
            return Err(DbError::InvalidTransition);
        }
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE work_unit SET status = ?, version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND status = 'open'",
        )
        .bind(input.status.to_string())
        .bind(&input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        if result.rows_affected() == 0 {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_unit WHERE id = ?)")
                    .bind(&input.id)
                    .fetch_one(&mut *tx)
                    .await?;
            return if exists {
                Err(DbError::VersionConflict)
            } else {
                Err(DbError::NotFound)
            };
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_work_unit(
            sqlx::query("SELECT * FROM work_unit WHERE id = ?")
                .bind(&input.id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn list_dependencies(&self, id: &str) -> Result<Vec<WorkUnitDependency>> {
        let rows = sqlx::query(
            "SELECT d.*, CASE WHEN p.status = 'completed'
                   AND (p.requires_integration = 0 OR EXISTS (
                       SELECT 1 FROM work_unit_integration i
                       WHERE i.work_unit_id = p.id AND i.task_id = p.task_id
                         AND i.outcome = 'success'
                   )) THEN 1 ELSE 0 END AS satisfied
             FROM work_unit_dependency d
             JOIN work_unit p ON p.id = d.depends_on_work_unit_id AND p.task_id = d.task_id
             WHERE d.work_unit_id = ?
             ORDER BY d.depends_on_work_unit_id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_dependency).collect()
    }

    async fn list_active_execution_ids(&self, id: &str) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar(
            "SELECT id FROM execution WHERE work_unit_id = ? AND status = 'running' ORDER BY created_at, id",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?)
    }

    async fn add_dependency(
        &self,
        input: AddWorkUnitDependency,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnitDependency>> {
        let mut tx = self.pool.begin().await?;
        let unit = sqlx::query("SELECT task_id, version, status FROM work_unit WHERE id = ?")
            .bind(&input.work_unit_id)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(DbError::NotFound)?;
        let task_id: String = unit.try_get("task_id")?;
        let version: i64 = unit.try_get("version")?;
        let status: String = unit.try_get("status")?;
        if version != input.expected_version {
            return Err(DbError::VersionConflict);
        }
        if status != "open" {
            return Err(DbError::InvalidTransition);
        }
        let (creator_kind, creator_id) = actor_columns(Some(&input.created_by));
        let result = sqlx::query(
            "INSERT INTO work_unit_dependency (
                 task_id, work_unit_id, depends_on_work_unit_id,
                 created_by_kind, created_by_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&task_id)
        .bind(&input.work_unit_id)
        .bind(&input.depends_on_work_unit_id)
        .bind(creator_kind.as_deref())
        .bind(creator_id.as_deref())
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        if result.rows_affected() == 0 {
            return Err(DbError::IdempotencyConflict);
        }
        let changed = sqlx::query(
            "UPDATE work_unit SET version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND status = 'open'",
        )
        .bind(&input.created_at)
        .bind(&input.work_unit_id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_dependency(
            sqlx::query(
                "SELECT d.*, CASE WHEN p.status = 'completed'
                       AND (p.requires_integration = 0 OR EXISTS (
                           SELECT 1 FROM work_unit_integration i
                           WHERE i.work_unit_id = p.id AND i.task_id = p.task_id
                             AND i.outcome = 'success'
                       )) THEN 1 ELSE 0 END AS satisfied
                 FROM work_unit_dependency d
                 JOIN work_unit p ON p.id = d.depends_on_work_unit_id AND p.task_id = d.task_id
                 WHERE d.work_unit_id = ? AND d.depends_on_work_unit_id = ?",
            )
            .bind(&input.work_unit_id)
            .bind(&input.depends_on_work_unit_id)
            .fetch_one(&mut *tx)
            .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn remove_dependency(
        &self,
        input: RemoveWorkUnitDependency,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnit>> {
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "DELETE FROM work_unit_dependency
             WHERE work_unit_id = ? AND depends_on_work_unit_id = ?
               AND task_id = (SELECT task_id FROM work_unit WHERE id = ?)",
        )
        .bind(&input.work_unit_id)
        .bind(&input.depends_on_work_unit_id)
        .bind(&input.work_unit_id)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        let changed = sqlx::query(
            "UPDATE work_unit SET version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND status = 'open'",
        )
        .bind(&input.updated_at)
        .bind(&input.work_unit_id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() == 0 {
            return Err(DbError::VersionConflict);
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_work_unit(
            sqlx::query("SELECT * FROM work_unit WHERE id = ?")
                .bind(&input.work_unit_id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn list_integrations(&self, id: &str) -> Result<Vec<WorkUnitIntegration>> {
        let rows = sqlx::query(
            "SELECT * FROM work_unit_integration WHERE work_unit_id = ? ORDER BY started_at DESC, id DESC",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_integration).collect()
    }

    async fn get_integration_by_id(&self, id: &str) -> Result<Option<WorkUnitIntegration>> {
        sqlx::query("SELECT * FROM work_unit_integration WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_integration)
            .transpose()
    }

    async fn get_integration_by_idempotency(
        &self,
        task_id: &str,
        key: &str,
    ) -> Result<Option<WorkUnitIntegration>> {
        sqlx::query(
            "SELECT * FROM work_unit_integration WHERE task_id = ? AND operation_idempotency_key = ?",
        )
        .bind(task_id)
        .bind(key)
        .fetch_optional(&self.pool)
        .await?
        .map(map_integration)
        .transpose()
    }

    async fn begin_integration(
        &self,
        input: CreateWorkUnitIntegration,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnitIntegration>> {
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO work_unit_integration (
                 id, task_id, work_unit_id, execution_id, source_workspace_id,
                 source_branch, source_sha, target_workspace_id, target_branch,
                 target_before_sha, operation_idempotency_key, outcome,
                 conflict_metadata_json, version, started_at, finished_at,
                 created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'running', NULL, 1, ?, NULL, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.work_unit_id)
        .bind(&input.execution_id)
        .bind(&input.source_workspace_id)
        .bind(&input.source_branch)
        .bind(&input.source_sha)
        .bind(&input.target_workspace_id)
        .bind(&input.target_branch)
        .bind(&input.target_before_sha)
        .bind(&input.operation_idempotency_key)
        .bind(&input.started_at)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_integration(
            sqlx::query("SELECT * FROM work_unit_integration WHERE id = ?")
                .bind(&input.id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }

    async fn record_integration(
        &self,
        input: RecordWorkUnitIntegration,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<WorkUnitIntegration>> {
        let after_sha = if input.outcome == WorkUnitIntegrationOutcome::Success {
            input.target_after_sha.as_deref()
        } else {
            None
        };
        let finished_at = input.finished_at.clone();
        let mut tx = self.pool.begin().await?;
        let result = sqlx::query(
            "UPDATE work_unit_integration SET outcome = ?, target_after_sha = ?,
                 conflict_metadata_json = ?, finished_at = ?, version = version + 1,
                 updated_at = ?
             WHERE id = ? AND version = ? AND outcome = 'running'",
        )
        .bind(input.outcome.to_string())
        .bind(after_sha)
        .bind(input.conflict_metadata_json.as_deref())
        .bind(&finished_at)
        .bind(&input.updated_at)
        .bind(&input.id)
        .bind(input.expected_version)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        if result.rows_affected() == 0 {
            let exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM work_unit_integration WHERE id = ?)",
            )
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await?;
            return if exists {
                Err(DbError::VersionConflict)
            } else {
                Err(DbError::NotFound)
            };
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        let record = map_integration(
            sqlx::query("SELECT * FROM work_unit_integration WHERE id = ?")
                .bind(&input.id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(CollaborationWrite { record, event })
    }
}

#[async_trait]
impl WorkUnitExecutionRepo for SqliteDb {
    async fn create_for_work_unit(
        &self,
        input: CreateWorkUnitExecution,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Execution>> {
        let mut tx = self.pool.begin().await?;
        let requires_integration = sqlx::query_scalar::<_, i64>(
            "SELECT requires_integration FROM work_unit WHERE id = ? AND task_id = ?",
        )
        .bind(&input.work_unit_id)
        .bind(&input.execution.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?
            != 0;
        if input.execution.status != ExecutionStatus::Running
            || requires_integration != input.workspace_lease.is_some()
        {
            return Err(DbError::Check(
                "WorkUnit Executions require a running Execution and repository work requires an exact WorkspaceLease"
                    .to_owned(),
            ));
        }
        if let Some(lease) = input.workspace_lease.as_ref() {
            let actor = input.execution.actor_ref.as_ref();
            let expected_lease_role = if input.execution.role == "reviewer" {
                "reviewer"
            } else {
                "worker"
            };
            if lease.task_id != input.execution.task_id
                || lease.work_unit_id.as_deref() != Some(input.work_unit_id.as_str())
                || lease.workspace_id != input.execution.workspace_id
                || lease.execution_id != input.execution.id
                || lease.role != expected_lease_role
                || Some(lease.assigned_principal_type.as_str())
                    != actor.map(|actor| actor.kind().to_string()).as_deref()
                || actor.map(|actor| actor.id()) != Some(lease.assigned_principal_id.as_str())
            {
                return Err(DbError::Check(
                    "WorkUnit WorkspaceLease does not match its Execution binding".to_owned(),
                ));
            }
        }
        let execution = Self::create_execution_in_tx(
            &mut tx,
            &input.execution,
            Some((&input.work_unit_id, input.work_unit_version)),
        )
        .await?;
        if let Some(lease) = input.workspace_lease {
            super::workspace_lease::insert_workspace_lease_in_tx(&mut tx, lease).await?;
        }
        let event = append_event(self, &mut tx, &event_input).await?;
        tx.commit().await?;
        Ok(CollaborationWrite {
            record: execution,
            event,
        })
    }
}

#[async_trait]
impl WorkUnitWorkspaceRepo for SqliteDb {
    async fn get_scope_by_id(&self, id: &str) -> Result<Option<WorkspaceScope>> {
        let row = sqlx::query("SELECT * FROM workspace_scope WHERE workspace_id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(|row| {
            Ok(WorkspaceScope {
                workspace_id: row.try_get("workspace_id")?,
                task_id: row.try_get("task_id")?,
                kind: parse_enum(row.try_get::<String, _>("scope_kind")?)?,
                work_unit_id: row.try_get("work_unit_id")?,
                created_at: row.try_get("created_at")?,
            })
        })
        .transpose()
    }

    async fn get_by_work_unit_id(&self, work_unit_id: &str) -> Result<Option<Workspace>> {
        sqlx::query(
            "SELECT w.* FROM workspace w
             JOIN workspace_scope s ON s.workspace_id = w.id AND s.task_id = w.task_id
             WHERE s.work_unit_id = ? AND s.scope_kind = 'work_unit'",
        )
        .bind(work_unit_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_workspace)
        .transpose()
    }

    async fn create_for_work_unit(&self, input: CreateWorkUnitWorkspace) -> Result<Workspace> {
        let ws = input.workspace;
        let mut tx = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO workspace (id, task_id, repo_id, worktree_path, branch, status,
                                    before_sha, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&ws.id)
        .bind(&ws.task_id)
        .bind(&ws.repo_id)
        .bind(&ws.worktree_path)
        .bind(&ws.branch)
        .bind(ws.status.to_string())
        .bind(ws.before_sha.as_deref())
        .bind(&ws.created_at)
        .bind(&ws.updated_at)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        sqlx::query(
            "INSERT INTO workspace_scope (workspace_id, task_id, scope_kind, work_unit_id, created_at)
             VALUES (?, ?, 'work_unit', ?, ?)",
        )
        .bind(&ws.id)
        .bind(&ws.task_id)
        .bind(&input.work_unit_id)
        .bind(&ws.created_at)
        .execute(&mut *tx)
        .await
        .map_err(work_unit_write_error)?;
        let record = map_workspace(
            sqlx::query("SELECT * FROM workspace WHERE id = ?")
                .bind(&ws.id)
                .fetch_one(&mut *tx)
                .await?,
        )?;
        tx.commit().await?;
        Ok(record)
    }

    async fn list_by_task(&self, task_id: &str) -> Result<Vec<Workspace>> {
        sqlx::query("SELECT * FROM workspace WHERE task_id = ? ORDER BY created_at, id")
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_workspace)
            .collect()
    }
}
