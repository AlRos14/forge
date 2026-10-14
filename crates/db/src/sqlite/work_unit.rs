use super::*;
use crate::FollowUpExecutionReceipt;

fn validate_blocking_metadata_execution_identity(
    error_annotation: Option<&str>,
    blocked_json: Option<&str>,
    blocking_execution_id: &str,
) -> Result<()> {
    if let Some(raw) = error_annotation {
        let annotation: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
            DbError::Check(format!("invalid Task blocking annotation JSON: {error}"))
        })?;
        if annotation
            .get("type")
            .and_then(serde_json::Value::as_str)
            .is_none()
            || annotation
                .get("blocked_execution_id")
                .and_then(serde_json::Value::as_str)
                != Some(blocking_execution_id)
        {
            return Err(DbError::VersionConflict);
        }
        if let Some(artifact) = annotation.get("artifact").filter(|value| !value.is_null()) {
            if artifact.get("kind").and_then(serde_json::Value::as_str) != Some("execution")
                || artifact.get("id").and_then(serde_json::Value::as_str)
                    != Some(blocking_execution_id)
            {
                return Err(DbError::VersionConflict);
            }
        }
    }
    if let Some(raw) = blocked_json {
        let metadata: serde_json::Value = serde_json::from_str(raw).map_err(|error| {
            DbError::Check(format!("invalid Task blocked metadata JSON: {error}"))
        })?;
        if metadata
            .get("execution_id")
            .and_then(serde_json::Value::as_str)
            != Some(blocking_execution_id)
        {
            return Err(DbError::VersionConflict);
        }
    }
    Ok(())
}

async fn validate_current_execution_block_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    task_id: &str,
    lifecycle_version: i64,
    lifecycle_reason_kind: Option<&str>,
    lifecycle_reason_ref: Option<&str>,
    blocking_execution_id: &str,
    blocking_work_unit_id: &str,
) -> Result<()> {
    if lifecycle_reason_kind.is_none() || lifecycle_reason_ref != Some(blocking_execution_id) {
        return Err(DbError::VersionConflict);
    }

    let transition = sqlx::query(
        "SELECT from_state, to_state, cause_kind, cause_ref, reason_kind, reason_ref
         FROM task_lifecycle_transition
         WHERE task_id = ? AND to_version = ?",
    )
    .bind(task_id)
    .bind(lifecycle_version)
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(transition) = transition {
        if transition.try_get::<String, _>("from_state")? == "blocked"
            || transition.try_get::<String, _>("to_state")? != "blocked"
            || transition.try_get::<String, _>("cause_kind")? != "execution"
            || transition
                .try_get::<Option<String>, _>("cause_ref")?
                .as_deref()
                != Some(blocking_execution_id)
            || transition
                .try_get::<Option<String>, _>("reason_kind")?
                .as_deref()
                != lifecycle_reason_kind
            || transition
                .try_get::<Option<String>, _>("reason_ref")?
                .as_deref()
                != Some(blocking_execution_id)
        {
            return Err(DbError::VersionConflict);
        }
    }

    let exact_blocking_execution: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM execution e
             JOIN work_unit wu ON wu.id = e.work_unit_id
             WHERE e.id = ? AND e.task_id = ? AND e.work_unit_id = ?
               AND e.status IN ('completed', 'failed', 'cancelled')
               AND wu.task_id = ? AND wu.status = 'open'
         )",
    )
    .bind(blocking_execution_id)
    .bind(task_id)
    .bind(blocking_work_unit_id)
    .bind(task_id)
    .fetch_one(&mut **tx)
    .await?;
    if !exact_blocking_execution {
        return Err(DbError::VersionConflict);
    }
    Ok(())
}

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

async fn insert_work_unit_execution_in_tx(
    db: &SqliteDb,
    tx: &mut Transaction<'_, Sqlite>,
    admission: CreateWorkUnitExecution,
    artifact_input_ids: Vec<String>,
    event_input: &CreateDomainEvent,
) -> Result<(Execution, DomainEvent)> {
    let requires_integration = sqlx::query_scalar::<_, i64>(
        "SELECT requires_integration FROM work_unit WHERE id = ? AND task_id = ?",
    )
    .bind(&admission.work_unit_id)
    .bind(&admission.execution.task_id)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(DbError::NotFound)?
        != 0;
    if admission.execution.status != ExecutionStatus::Running
        || requires_integration != admission.workspace_lease.is_some()
    {
        return Err(DbError::Check(
            "WorkUnit Executions require a running Execution and repository work requires an exact WorkspaceLease"
                .to_owned(),
        ));
    }
    if let Some(lease) = admission.workspace_lease.as_ref() {
        let actor = admission.execution.actor_ref.as_ref();
        let expected_lease_role = if admission.execution.role == "reviewer" {
            "reviewer"
        } else {
            "worker"
        };
        if lease.task_id != admission.execution.task_id
            || lease.work_unit_id.as_deref() != Some(admission.work_unit_id.as_str())
            || lease.workspace_id != admission.execution.workspace_id
            || lease.execution_id != admission.execution.id
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

    let execution = SqliteDb::create_execution_in_tx(
        tx,
        &admission.execution,
        Some((&admission.work_unit_id, admission.work_unit_version)),
    )
    .await?;
    for artifact_id in artifact_input_ids {
        super::collaboration::pin_execution_artifact_input_in_tx(
            tx,
            &admission.execution.id,
            &artifact_id,
            &admission.execution.created_at,
        )
        .await?;
    }
    if let Some(lease) = admission.workspace_lease {
        super::workspace_lease::insert_workspace_lease_in_tx(tx, lease).await?;
    }
    let event = append_event(db, tx, event_input).await?;
    Ok((execution, event))
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
        self.create_for_work_unit_with_artifact_inputs(input, Vec::new(), event_input)
            .await
    }

    async fn create_for_work_unit_with_artifact_inputs(
        &self,
        input: CreateWorkUnitExecution,
        artifact_input_ids: Vec<String>,
        event_input: CreateDomainEvent,
    ) -> Result<CollaborationWrite<Execution>> {
        let mut tx = self.pool.begin().await?;
        let (execution, event) = insert_work_unit_execution_in_tx(
            self,
            &mut tx,
            input,
            artifact_input_ids,
            &event_input,
        )
        .await?;
        tx.commit().await?;
        Ok(CollaborationWrite {
            record: execution,
            event,
        })
    }

    async fn get_follow_up_execution_receipt(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<FollowUpExecutionReceipt>> {
        sqlx::query(
            "SELECT idempotency_key, parent_execution_id, request_digest, execution_id
             FROM execution_follow_up_receipt WHERE idempotency_key = ?",
        )
        .bind(idempotency_key)
        .fetch_optional(&self.pool)
        .await?
        .map(|row| {
            Ok(FollowUpExecutionReceipt {
                idempotency_key: row.try_get("idempotency_key")?,
                parent_execution_id: row.try_get("parent_execution_id")?,
                request_digest: row.try_get("request_digest")?,
                execution_id: row.try_get("execution_id")?,
            })
        })
        .transpose()
    }

    async fn admit_with_lifecycle_and_artifact_inputs(
        &self,
        input: AdmitWorkUnitExecutionWithLifecycle,
        artifact_input_ids: Vec<String>,
        execution_event_input: CreateDomainEvent,
    ) -> Result<WorkUnitExecutionLifecycleWrite> {
        let admission = input.admission;
        let task_id = admission.execution.task_id.as_str();
        if let Some(receipt) = input.follow_up_receipt.as_ref() {
            if receipt.idempotency_key.trim().is_empty()
                || receipt.idempotency_key.len() > 256
                || receipt.request_digest.len() != 64
                || !receipt
                    .request_digest
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit())
                || admission.execution.parent_execution_id.as_deref()
                    != Some(receipt.parent_execution_id.as_str())
            {
                return Err(DbError::Check(
                    "follow-up receipt must bind a bounded key, digest, and exact parent"
                        .to_owned(),
                ));
            }
        }
        if admission.execution.status != ExecutionStatus::Running
            || admission.execution.purpose.is_none()
            || execution_event_input.event_type != "execution.started"
            || execution_event_input.entity_type != "execution"
            || execution_event_input.entity_id != admission.execution.id
            || execution_event_input.scope_type != "task"
            || execution_event_input.scope_id != task_id
        {
            return Err(DbError::Check(
                "ordinary WorkUnit admission requires one exact Running Execution and start event"
                    .to_owned(),
            ));
        }

        let transition_count = i64::try_from(input.lifecycle_transitions.len())
            .map_err(|_| DbError::Check("too many lifecycle transitions".to_owned()))?;
        if transition_count > 2 {
            return Err(DbError::Check(
                "ordinary WorkUnit admission may apply at most two lifecycle transitions"
                    .to_owned(),
            ));
        }
        let expected_final_state = if transition_count == 0 {
            crate::TaskLifecycleState::Active
        } else {
            input.lifecycle_transitions.last().unwrap().to_state
        };
        if expected_final_state != crate::TaskLifecycleState::Active {
            return Err(DbError::Check(
                "ordinary WorkUnit admission must preserve or activate TaskLifecycle".to_owned(),
            ));
        }

        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        if let Some(receipt) = input.follow_up_receipt.as_ref() {
            let prior = sqlx::query(
                "SELECT parent_execution_id, request_digest, execution_id
                 FROM execution_follow_up_receipt WHERE idempotency_key = ?",
            )
            .bind(&receipt.idempotency_key)
            .fetch_optional(&mut *tx)
            .await?;
            if let Some(prior) = prior {
                let parent_execution_id: String = prior.try_get("parent_execution_id")?;
                let request_digest: String = prior.try_get("request_digest")?;
                let execution_id: String = prior.try_get("execution_id")?;
                if parent_execution_id != receipt.parent_execution_id
                    || request_digest != receipt.request_digest
                {
                    return Err(DbError::IdempotencyConflict);
                }
                let execution = sqlx::query(
                    "SELECT * FROM execution
                     WHERE id = ? AND parent_execution_id = ? AND task_id = ?",
                )
                .bind(&execution_id)
                .bind(&receipt.parent_execution_id)
                .bind(task_id)
                .fetch_optional(&mut *tx)
                .await?
                .map(super::map_execution)
                .transpose()?
                .ok_or_else(|| {
                    DbError::Check(
                        "follow-up receipt does not resolve to its exact child Execution"
                            .to_owned(),
                    )
                })?;
                let has_start_event: bool = sqlx::query_scalar(
                    "SELECT EXISTS(
                        SELECT 1 FROM domain_event
                        WHERE event_type = 'execution.started'
                          AND entity_type = 'execution' AND entity_id = ?
                          AND scope_type = 'task' AND scope_id = ?
                    )",
                )
                .bind(&execution.id)
                .bind(task_id)
                .fetch_one(&mut *tx)
                .await?;
                if !has_start_event {
                    return Err(DbError::Check(
                        "follow-up receipt child has no durable start event".to_owned(),
                    ));
                }
                tx.commit().await?;
                return Ok(WorkUnitExecutionLifecycleWrite {
                    execution,
                    lifecycle_events: Vec::new(),
                    execution_event: None,
                    idempotent_replay: true,
                });
            }
        }
        let task_row = sqlx::query(
            "SELECT version, error_annotation, blocked_json, failed_json, metadata_json
             FROM task WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let task_version: i64 = task_row.try_get("version")?;
        if task_version != input.expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: input.expected_task_version,
                actual: task_version,
            });
        }
        let lifecycle_row = sqlx::query(
            "SELECT state, version, reason_kind, reason_ref
             FROM task_lifecycle WHERE task_id = ?",
        )
        .bind(task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let current_state =
            parse_enum::<crate::TaskLifecycleState>(lifecycle_row.try_get("state")?)?;
        let current_lifecycle_version: i64 = lifecycle_row.try_get("version")?;
        let current_lifecycle_reason_kind: Option<String> = lifecycle_row.try_get("reason_kind")?;
        let current_lifecycle_reason_ref: Option<String> = lifecycle_row.try_get("reason_ref")?;
        if current_state != input.expected_lifecycle_state
            || current_lifecycle_version != input.expected_lifecycle_version
        {
            return Err(DbError::VersionConflict);
        }
        let retry_budget_exhausted: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                 SELECT 1 FROM task_failure_retry_receipt receipt
                 WHERE receipt.task_id = ? AND receipt.disposition = 'exhausted'
                   AND NOT EXISTS (
                       SELECT 1 FROM task_retry_override override
                       WHERE override.exhaustion_receipt_id = receipt.id
                         AND override.task_id = receipt.task_id
                         AND override.failure_kind = receipt.failure_kind
                   )
             )",
        )
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        if retry_budget_exhausted {
            return Err(DbError::Check(
                "Task retry budget is exhausted; WorkUnit Execution admission is fenced".to_owned(),
            ));
        }
        match (current_state, input.lifecycle_transitions.as_slice()) {
            (crate::TaskLifecycleState::Active, [])
            | (crate::TaskLifecycleState::Ready | crate::TaskLifecycleState::Blocked, [_])
            | (crate::TaskLifecycleState::Backlog, [_, _]) => {}
            _ => {
                return Err(DbError::Check(
                    "ordinary WorkUnit admission lifecycle path is outside the bounded activation contract"
                        .to_owned(),
                ));
            }
        }
        let mut planned_state = current_state;
        let mut planned_lifecycle_version = current_lifecycle_version;
        let mut planned_task_version = input.expected_task_version;
        for transition in &input.lifecycle_transitions {
            let expected_idempotency_key = match planned_state {
                crate::TaskLifecycleState::Backlog => {
                    format!("execution-start:{task_id}:backlog:{planned_lifecycle_version}")
                }
                crate::TaskLifecycleState::Ready | crate::TaskLifecycleState::Blocked => {
                    format!("execution-start:{task_id}:active:{planned_lifecycle_version}")
                }
                _ => String::new(),
            };
            let expected_reason_ref = match planned_state {
                crate::TaskLifecycleState::Backlog => format!("task:{task_id}:backlog-to-ready"),
                crate::TaskLifecycleState::Ready | crate::TaskLifecycleState::Blocked => {
                    format!("task:{task_id}:active")
                }
                _ => String::new(),
            };
            let expected_to_state = if planned_state == crate::TaskLifecycleState::Backlog {
                crate::TaskLifecycleState::Ready
            } else {
                crate::TaskLifecycleState::Active
            };
            let event_payload: serde_json::Value =
                serde_json::from_str(&transition.event.payload_json).map_err(|error| {
                    DbError::Check(format!("invalid lifecycle event payload: {error}"))
                })?;
            if transition.task_id != task_id
                || transition.expected_state != planned_state
                || transition.expected_lifecycle_version != planned_lifecycle_version
                || transition.expected_task_version != planned_task_version
                || transition.idempotency_key != expected_idempotency_key
                || transition.to_state != expected_to_state
                || transition.cause_kind != "system"
                || transition.cause_ref.as_deref() != Some("executor")
                || transition.reason_kind.as_deref() != Some("execution_start")
                || transition.reason_ref.as_deref() != Some(expected_reason_ref.as_str())
                || transition.gate_evaluation_id.is_some()
                || transition.event.event_type != "task.lifecycle_changed"
                || transition.event.entity_type != "task"
                || transition.event.entity_id != task_id
                || transition.event.scope_type != "task"
                || transition.event.scope_id != task_id
                || transition.event.correlation_id != transition.idempotency_key
                || transition.event.dedupe_key.as_deref()
                    != Some(
                        format!("task-lifecycle:{task_id}:{}", transition.idempotency_key).as_str(),
                    )
                || event_payload
                    .get("task_id")
                    .and_then(serde_json::Value::as_str)
                    != Some(task_id)
                || event_payload
                    .get("from_state")
                    .and_then(serde_json::Value::as_str)
                    != Some(planned_state.to_string().as_str())
                || event_payload
                    .get("to_state")
                    .and_then(serde_json::Value::as_str)
                    != Some(expected_to_state.to_string().as_str())
                || event_payload
                    .get("from_version")
                    .and_then(serde_json::Value::as_i64)
                    != Some(planned_lifecycle_version)
                || event_payload
                    .get("to_version")
                    .and_then(serde_json::Value::as_i64)
                    != planned_lifecycle_version.checked_add(1)
                || event_payload
                    .get("cause_kind")
                    .and_then(serde_json::Value::as_str)
                    != Some("system")
                || event_payload
                    .get("cause_ref")
                    .and_then(serde_json::Value::as_str)
                    != Some("executor")
                || event_payload
                    .get("reason_kind")
                    .and_then(serde_json::Value::as_str)
                    != Some("execution_start")
                || event_payload
                    .get("reason_ref")
                    .and_then(serde_json::Value::as_str)
                    != Some(expected_reason_ref.as_str())
            {
                return Err(DbError::Check(
                    "ordinary WorkUnit admission lifecycle receipt does not match its exact activation edge"
                        .to_owned(),
                ));
            }
            planned_state = transition.to_state;
            planned_lifecycle_version = planned_lifecycle_version
                .checked_add(1)
                .ok_or_else(|| DbError::Check("TaskLifecycle version overflow".to_owned()))?;
            planned_task_version = planned_task_version
                .checked_add(1)
                .ok_or_else(|| DbError::Check("Task version overflow".to_owned()))?;
        }
        if input.lifecycle_transitions.is_empty()
            && (current_state != crate::TaskLifecycleState::Active
                || current_lifecycle_version != input.expected_lifecycle_version)
        {
            return Err(DbError::VersionConflict);
        }

        if let Some(parent_execution_id) = admission.execution.parent_execution_id.as_deref() {
            let parent_is_exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM execution
                    WHERE id = ? AND task_id = ? AND work_unit_id = ?
                      AND workspace_id IS ?
                      AND status IN ('completed', 'failed', 'cancelled')
                )",
            )
            .bind(parent_execution_id)
            .bind(task_id)
            .bind(&admission.work_unit_id)
            .bind(admission.execution.workspace_id.as_deref())
            .fetch_one(&mut *tx)
            .await?;
            if !parent_is_exact {
                return Err(DbError::VersionConflict);
            }
        }

        match (current_state, input.clear_blocking_metadata.as_ref()) {
            (crate::TaskLifecycleState::Blocked, Some(clear)) => {
                if admission.execution.parent_execution_id.as_deref()
                    != Some(clear.blocking_execution_id.as_str())
                    || admission.work_unit_id != clear.blocking_work_unit_id
                    || current_lifecycle_reason_kind.as_deref()
                        != Some(clear.expected_lifecycle_reason_kind.as_str())
                    || task_row.try_get::<Option<String>, _>("error_annotation")?
                        != clear.expected_error_annotation
                    || task_row.try_get::<Option<String>, _>("blocked_json")?
                        != clear.expected_blocked_json
                    || task_row.try_get::<Option<String>, _>("metadata_json")?
                        != clear.expected_metadata_json
                    || task_row
                        .try_get::<Option<String>, _>("failed_json")?
                        .is_some()
                {
                    return Err(DbError::VersionConflict);
                }
                validate_blocking_metadata_execution_identity(
                    clear.expected_error_annotation.as_deref(),
                    clear.expected_blocked_json.as_deref(),
                    &clear.blocking_execution_id,
                )?;
                validate_current_execution_block_in_tx(
                    &mut tx,
                    task_id,
                    current_lifecycle_version,
                    current_lifecycle_reason_kind.as_deref(),
                    current_lifecycle_reason_ref.as_deref(),
                    &clear.blocking_execution_id,
                    &clear.blocking_work_unit_id,
                )
                .await?;
            }
            (crate::TaskLifecycleState::Blocked, None) => {
                return Err(DbError::Check(
                    "Blocked Task activation must resolve its exact current Execution block"
                        .to_owned(),
                ));
            }
            (_, Some(_)) => {
                return Err(DbError::Check(
                    "blocking metadata can only be cleared while resolving a Blocked Task"
                        .to_owned(),
                ));
            }
            (_, None) => {}
        }

        let mut lifecycle_events = Vec::with_capacity(input.lifecycle_transitions.len());
        for transition in &input.lifecycle_transitions {
            let (_, event) =
                super::gate_lifecycle::record_lifecycle_transition_in_tx(self, &mut tx, transition)
                    .await?;
            let updated_task = sqlx::query(
                "UPDATE task SET version = version + 1, updated_at = ?
                 WHERE id = ? AND version = ? AND deleted_at IS NULL",
            )
            .bind(&transition.updated_at)
            .bind(task_id)
            .bind(transition.expected_task_version)
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if updated_task != 1 {
                return Err(DbError::VersionConflict);
            }
            lifecycle_events.push(event);
        }

        let mut final_task_version = planned_task_version;
        if let Some(clear) = input.clear_blocking_metadata {
            let updated_task = sqlx::query(
                "UPDATE task
                 SET error_annotation = NULL, blocked_json = NULL,
                     metadata_json = ?, version = version + 1, updated_at = ?
                 WHERE id = ? AND version = ? AND error_annotation IS ?
                   AND blocked_json IS ? AND failed_json IS NULL
                   AND metadata_json IS ? AND deleted_at IS NULL",
            )
            .bind(clear.replacement_metadata_json.as_deref())
            .bind(&admission.execution.created_at)
            .bind(task_id)
            .bind(final_task_version)
            .bind(clear.expected_error_annotation.as_deref())
            .bind(clear.expected_blocked_json.as_deref())
            .bind(clear.expected_metadata_json.as_deref())
            .execute(&mut *tx)
            .await?
            .rows_affected();
            if updated_task != 1 {
                return Err(DbError::VersionConflict);
            }
            final_task_version = final_task_version
                .checked_add(1)
                .ok_or_else(|| DbError::Check("Task version overflow".to_owned()))?;
        }
        if let Some(lease) = admission.workspace_lease.as_ref() {
            if lease.task_version != final_task_version {
                return Err(DbError::Check(
                    "WorkUnit WorkspaceLease Task version does not match lifecycle admission"
                        .to_owned(),
                ));
            }
        }

        let (execution, execution_event) = insert_work_unit_execution_in_tx(
            self,
            &mut tx,
            admission,
            artifact_input_ids,
            &execution_event_input,
        )
        .await?;
        if let Some(receipt) = input.follow_up_receipt {
            sqlx::query(
                "INSERT INTO execution_follow_up_receipt
                    (idempotency_key, parent_execution_id, request_digest, execution_id, created_at)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&receipt.idempotency_key)
            .bind(&receipt.parent_execution_id)
            .bind(&receipt.request_digest)
            .bind(&execution.id)
            .bind(&execution.created_at)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(WorkUnitExecutionLifecycleWrite {
            execution,
            lifecycle_events,
            execution_event: Some(execution_event),
            idempotent_replay: false,
        })
    }

    async fn admit_recovery_for_work_unit_with_artifact_inputs(
        &self,
        input: RecoverWorkUnitExecution,
        artifact_input_ids: Vec<String>,
        execution_event_input: CreateDomainEvent,
    ) -> Result<RecoveryWorkUnitExecutionWrite> {
        let admission = input.admission;
        let transition = input.lifecycle_transition;
        let parent_execution_id = admission
            .execution
            .parent_execution_id
            .as_deref()
            .ok_or_else(|| {
                DbError::Check("recovery Execution requires exact parent lineage".to_owned())
            })?;
        let admitted_task_version = transition
            .expected_task_version
            .checked_add(1)
            .ok_or_else(|| DbError::Check("Task version overflow during recovery".to_owned()))?;
        if transition.task_id != admission.execution.task_id
            || transition.expected_state != crate::TaskLifecycleState::Blocked
            || transition.to_state != crate::TaskLifecycleState::Active
            || transition.cause_kind != "execution"
            || transition.cause_ref.as_deref() != Some(parent_execution_id)
            || admission.execution.status != ExecutionStatus::Running
            || execution_event_input.event_type != "execution.started"
            || execution_event_input.entity_type != "execution"
            || execution_event_input.entity_id != admission.execution.id
            || execution_event_input.scope_type != "task"
            || execution_event_input.scope_id != admission.execution.task_id
        {
            return Err(DbError::Check(
                "recovery lifecycle, parent, and Execution admission must identify one exact target"
                    .to_owned(),
            ));
        }

        // Serialize the state read and every durable admission write. If a
        // WorkUnit, membership, Workspace, lease, or ArtifactInput check later
        // rejects the new Execution, this transaction also rolls back the
        // lifecycle transition and clearing of the old recovery annotation.
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let task_row = sqlx::query(
            "SELECT version, error_annotation, blocked_json, failed_json, metadata_json
             FROM task WHERE id = ? AND deleted_at IS NULL",
        )
        .bind(&admission.execution.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let task_version: i64 = task_row.try_get("version")?;
        if task_version != transition.expected_task_version {
            return Err(DbError::TaskVersionConflict {
                expected: transition.expected_task_version,
                actual: task_version,
            });
        }
        if task_row.try_get::<Option<String>, _>("error_annotation")?
            != input.expected_error_annotation
            || task_row.try_get::<Option<String>, _>("blocked_json")? != input.expected_blocked_json
            || task_row.try_get::<Option<String>, _>("metadata_json")?
                != input.expected_metadata_json
            || task_row
                .try_get::<Option<String>, _>("failed_json")?
                .is_some()
        {
            return Err(DbError::VersionConflict);
        }
        let lifecycle_row = sqlx::query(
            "SELECT state, version, reason_kind, reason_ref
             FROM task_lifecycle WHERE task_id = ?",
        )
        .bind(&admission.execution.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?;
        let lifecycle_state =
            parse_enum::<crate::TaskLifecycleState>(lifecycle_row.try_get("state")?)?;
        let lifecycle_version: i64 = lifecycle_row.try_get("version")?;
        let lifecycle_reason_kind: Option<String> = lifecycle_row.try_get("reason_kind")?;
        let lifecycle_reason_ref: Option<String> = lifecycle_row.try_get("reason_ref")?;
        if lifecycle_state != crate::TaskLifecycleState::Blocked
            || lifecycle_version != transition.expected_lifecycle_version
        {
            return Err(DbError::VersionConflict);
        }
        validate_blocking_metadata_execution_identity(
            input.expected_error_annotation.as_deref(),
            input.expected_blocked_json.as_deref(),
            parent_execution_id,
        )?;
        validate_current_execution_block_in_tx(
            &mut tx,
            &admission.execution.task_id,
            lifecycle_version,
            lifecycle_reason_kind.as_deref(),
            lifecycle_reason_ref.as_deref(),
            parent_execution_id,
            &admission.work_unit_id,
        )
        .await?;
        let parent_is_exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM execution
                WHERE id = ? AND task_id = ? AND work_unit_id = ?
                  AND status IN ('completed', 'failed', 'cancelled')
            )",
        )
        .bind(parent_execution_id)
        .bind(&admission.execution.task_id)
        .bind(&admission.work_unit_id)
        .fetch_one(&mut *tx)
        .await?;
        if !parent_is_exact {
            return Err(DbError::VersionConflict);
        }

        let requires_integration = sqlx::query_scalar::<_, i64>(
            "SELECT requires_integration FROM work_unit WHERE id = ? AND task_id = ?",
        )
        .bind(&admission.work_unit_id)
        .bind(&admission.execution.task_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(DbError::NotFound)?
            != 0;
        if requires_integration != admission.workspace_lease.is_some() {
            return Err(DbError::Check(
                "recovery repository work requires its exact WorkspaceLease".to_owned(),
            ));
        }
        if let Some(lease) = admission.workspace_lease.as_ref() {
            let actor = admission.execution.actor_ref.as_ref();
            let expected_lease_role = if admission.execution.role == "reviewer" {
                "reviewer"
            } else {
                "worker"
            };
            if lease.task_id != admission.execution.task_id
                || lease.work_unit_id.as_deref() != Some(admission.work_unit_id.as_str())
                || lease.workspace_id != admission.execution.workspace_id
                || lease.execution_id != admission.execution.id
                || lease.task_version != admitted_task_version
                || lease.role != expected_lease_role
                || Some(lease.assigned_principal_type.as_str())
                    != actor.map(|actor| actor.kind().to_string()).as_deref()
                || actor.map(|actor| actor.id()) != Some(lease.assigned_principal_id.as_str())
            {
                return Err(DbError::Check(
                    "recovery WorkspaceLease does not match its exact Task and Execution"
                        .to_owned(),
                ));
            }
        }

        let (_lifecycle, lifecycle_event) =
            super::gate_lifecycle::record_lifecycle_transition_in_tx(self, &mut tx, &transition)
                .await?;
        let updated_task = sqlx::query(
            "UPDATE task
             SET error_annotation = NULL, blocked_json = NULL,
                 version = version + 1, updated_at = ?
             WHERE id = ? AND version = ? AND error_annotation IS ?
               AND blocked_json IS ? AND failed_json IS NULL
               AND metadata_json IS ? AND deleted_at IS NULL",
        )
        .bind(&transition.updated_at)
        .bind(&transition.task_id)
        .bind(transition.expected_task_version)
        .bind(input.expected_error_annotation.as_deref())
        .bind(input.expected_blocked_json.as_deref())
        .bind(input.expected_metadata_json.as_deref())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated_task != 1 {
            return Err(DbError::VersionConflict);
        }

        let (execution, execution_event) = insert_work_unit_execution_in_tx(
            self,
            &mut tx,
            admission,
            artifact_input_ids,
            &execution_event_input,
        )
        .await?;
        tx.commit().await?;
        Ok(RecoveryWorkUnitExecutionWrite {
            execution,
            lifecycle_event,
            execution_event,
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
