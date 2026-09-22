use super::*;

pub(crate) fn map_harness_session(row: SqliteRow) -> Result<HarnessSession> {
    Ok(HarnessSession {
        id: row.try_get("id")?,
        agent_id: row.try_get("agent_id")?,
        harness_kind: row.try_get("harness_kind")?,
        external_session_id: row.try_get("external_session_id")?,
        profile_id: row.try_get("profile_id")?,
        profile_snapshot_json: row.try_get("profile_snapshot_json")?,
        capabilities_snapshot_json: row.try_get("capabilities_snapshot_json")?,
        workspace_id: row.try_get("workspace_id")?,
        status: parse_enum(row.try_get::<String, _>("status")?)?,
        predecessor_session_id: row.try_get("predecessor_session_id")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
        last_activity_at: row.try_get("last_activity_at")?,
    })
}

pub(crate) async fn create_pending_harness_session_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    input: &CreateExecution,
) -> Result<Option<String>> {
    let Some(ActorRef::Agent(actor_agent_id)) = input.actor_ref.as_ref() else {
        return Ok(None);
    };
    if input.harness_session_id.is_some() || input.executor_config_snapshot_json.is_none() {
        return Ok(input.harness_session_id.clone());
    }
    if input.agent_id.as_deref() != Some(actor_agent_id.as_str()) {
        return Err(DbError::Check(
            "Execution ActorRef and agent compatibility projection disagree".to_owned(),
        ));
    }

    let snapshot_json = input
        .executor_config_snapshot_json
        .as_deref()
        .unwrap_or("{}");
    let snapshot = serde_json::from_str::<serde_json::Value>(snapshot_json).unwrap_or_default();
    let harness_kind = snapshot
        .get("executor_type")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("legacy")
        .to_owned();
    let profile_id = snapshot
        .get("profile_id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let capabilities_snapshot_json = snapshot
        .get("capabilities")
        .map(ToString::to_string)
        .unwrap_or_else(|| "{}".to_owned());
    let id = new_uuid_v4();
    sqlx::query(
        "INSERT INTO harness_session (
             id, agent_id, harness_kind, external_session_id, profile_id,
             profile_snapshot_json, capabilities_snapshot_json, workspace_id,
             status, predecessor_session_id, created_at, updated_at, last_activity_at
         ) VALUES (?, ?, ?, NULL, ?, ?, ?, ?, 'pending', NULL, ?, ?, NULL)",
    )
    .bind(&id)
    .bind(actor_agent_id)
    .bind(&harness_kind)
    .bind(profile_id.as_deref())
    .bind(snapshot_json)
    .bind(&capabilities_snapshot_json)
    .bind(input.workspace_id.as_deref())
    .bind(&input.created_at)
    .bind(&input.updated_at)
    .execute(&mut **transaction)
    .await?;
    Ok(Some(id))
}

pub(crate) async fn validate_execution_harness_session_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    input: &CreateExecution,
) -> Result<()> {
    let Some(session_id) = input.harness_session_id.as_deref() else {
        return Ok(());
    };
    let Some(ActorRef::Agent(agent_id)) = input.actor_ref.as_ref() else {
        return Err(DbError::Check(
            "Human Executions cannot reference a HarnessSession".to_owned(),
        ));
    };
    let row = sqlx::query(
        "SELECT agent_id, status, workspace_id
         FROM harness_session WHERE id = ?",
    )
    .bind(session_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DbError::NotFound)?;
    let session_agent_id: String = row.try_get("agent_id")?;
    let status: HarnessSessionStatus = parse_enum(row.try_get::<String, _>("status")?)?;
    let session_workspace_id: Option<String> = row.try_get("workspace_id")?;
    if session_agent_id != *agent_id {
        return Err(DbError::Check(
            "HarnessSession belongs to a different Agent".to_owned(),
        ));
    }
    if !matches!(
        status,
        HarnessSessionStatus::Pending | HarnessSessionStatus::Active
    ) {
        return Err(DbError::Check("HarnessSession is not reusable".to_owned()));
    }
    if session_workspace_id.is_some()
        && session_workspace_id.as_deref() != input.workspace_id.as_deref()
    {
        return Err(DbError::Check(
            "HarnessSession workspace is incompatible with Execution".to_owned(),
        ));
    }
    Ok(())
}

#[async_trait]
impl HarnessSessionRepo for SqliteDb {
    async fn create(&self, input: CreateHarnessSession) -> Result<HarnessSession> {
        let mut transaction = self.pool.begin().await?;
        sqlx::query(
            "INSERT INTO harness_session (
                 id, agent_id, harness_kind, external_session_id, profile_id,
                 profile_snapshot_json, capabilities_snapshot_json, workspace_id,
                 status, predecessor_session_id, created_at, updated_at, last_activity_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.agent_id)
        .bind(&input.harness_kind)
        .bind(input.external_session_id.as_deref())
        .bind(input.profile_id.as_deref())
        .bind(&input.profile_snapshot_json)
        .bind(&input.capabilities_snapshot_json)
        .bind(input.workspace_id.as_deref())
        .bind(input.status.to_string())
        .bind(input.predecessor_session_id.as_deref())
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .bind(input.last_activity_at.as_deref())
        .execute(&mut *transaction)
        .await?;
        transaction.commit().await?;
        HarnessSessionRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<HarnessSession>> {
        sqlx::query("SELECT * FROM harness_session WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_harness_session)
            .transpose()
    }

    async fn find_reusable(
        &self,
        agent_id: &str,
        harness_kind: &str,
        external_session_id: &str,
    ) -> Result<Option<HarnessSession>> {
        sqlx::query(
            "SELECT * FROM harness_session
             WHERE agent_id = ? AND harness_kind = ? AND external_session_id = ?
               AND status IN ('pending', 'active')
             LIMIT 1",
        )
        .bind(agent_id)
        .bind(harness_kind)
        .bind(external_session_id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_harness_session)
        .transpose()
    }

    async fn update(&self, input: UpdateHarnessSession) -> Result<HarnessSession> {
        let current = HarnessSessionRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let requested_external_session_id = input.external_session_id.clone();
        let external_session_id = requested_external_session_id
            .clone()
            .unwrap_or_else(|| current.external_session_id.clone());
        if let (Some(current_external), Some(next_external)) = (
            current.external_session_id.as_deref(),
            external_session_id.as_deref(),
        ) {
            if current_external != next_external {
                return Err(DbError::Check(
                    "HarnessSession external identity is immutable once known".to_owned(),
                ));
            }
        }
        if current.external_session_id.is_some()
            && requested_external_session_id.is_some_and(|value| value.is_none())
        {
            return Err(DbError::Check(
                "HarnessSession external identity cannot be cleared once known".to_owned(),
            ));
        }
        let status = input
            .status
            .clone()
            .unwrap_or_else(|| current.status.clone());
        if matches!(
            current.status,
            HarnessSessionStatus::Ended | HarnessSessionStatus::Failed
        ) && matches!(
            status,
            HarnessSessionStatus::Pending | HarnessSessionStatus::Active
        ) {
            return Err(DbError::Check(
                "ended or failed HarnessSession cannot become reusable".to_owned(),
            ));
        }
        let last_activity_at = input.last_activity_at.unwrap_or(current.last_activity_at);
        sqlx::query(
            "UPDATE harness_session
             SET external_session_id = ?, status = ?, last_activity_at = ?, updated_at = ?
             WHERE id = ?",
        )
        .bind(external_session_id.as_deref())
        .bind(status.to_string())
        .bind(last_activity_at.as_deref())
        .bind(&input.updated_at)
        .bind(&input.id)
        .execute(&self.pool)
        .await?;
        HarnessSessionRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }
}
