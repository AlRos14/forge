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

/// PR2 only pre-materializes a pending generic session when the current
/// executor family can actually report an external continuity id. This is a
/// narrow admission hint, not the PR3 capability model: unknown harness kinds
/// remain opaque and can still materialize a session when a result supplies an
/// external identity.
fn may_establish_external_session(snapshot: &serde_json::Value) -> bool {
    // An ordered fallback route has not established continuity until its
    // winner is persisted.  Do not freeze the primary candidate's harness or
    // profile snapshot into a pending generic session before routing resolves.
    if let Some(routing) = snapshot.get("routing") {
        let selected_candidate_key = routing
            .as_object()
            .and_then(|routing| routing.get("selected_candidate_key"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty());
        if selected_candidate_key.is_none() {
            return false;
        }
    }
    let Some(executor_type) = snapshot
        .get("executor_type")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
    else {
        return false;
    };
    !(executor_type.eq_ignore_ascii_case("shell")
        || executor_type.eq_ignore_ascii_case("gemini")
        || executor_type.eq_ignore_ascii_case("null"))
}

#[cfg(test)]
mod tests {
    use super::may_establish_external_session;

    #[test]
    fn unresolved_ordered_fallback_does_not_admit_pending_session() {
        let snapshot = serde_json::json!({
            "executor_type": "codex",
            "config": {"profile": "account-a"},
            "routing": {
                "policy": "ordered_fallback_v1",
                "candidates": [
                    {"executor_type": "codex", "config": {"profile": "account-a"}},
                    {"executor_type": "cursor", "config": {"profile": "account-b"}}
                ]
            }
        });

        assert!(!may_establish_external_session(&snapshot));
    }

    #[test]
    fn resolved_ordered_fallback_admits_the_resolved_candidate_snapshot() {
        let snapshot = serde_json::json!({
            "executor_type": "cursor",
            "config": {"profile": "account-b"},
            "routing": {
                "policy": "ordered_fallback_v1",
                "selected_candidate_key": "cursor:account-b",
                "candidates": [
                    {"executor_type": "codex", "config": {"profile": "account-a"}},
                    {"executor_type": "cursor", "config": {"profile": "account-b"}}
                ]
            }
        });

        assert!(may_establish_external_session(&snapshot));
    }
}

pub(crate) async fn profile_id_for_snapshot_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    snapshot: &serde_json::Value,
) -> Result<Option<String>> {
    let Some(profile_id) = snapshot
        .get("profile_id")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    sqlx::query_scalar(
        "SELECT id FROM agent_profile WHERE id = ? AND identity_id = ? LIMIT 1",
    )
    .bind(profile_id)
    .bind(agent_id)
    .fetch_optional(&mut **transaction)
    .await
    .map_err(Into::into)
}

pub(crate) async fn create_pending_harness_session_in_tx(
    transaction: &mut Transaction<'_, Sqlite>,
    input: &CreateExecution,
) -> Result<Option<String>> {
    let Some(ActorRef::Agent(actor_agent_id)) = input.actor_ref.as_ref() else {
        return Ok(None);
    };
    if input.harness_session_id.is_some()
        || input.executor_config_snapshot_json.is_none()
        || input.status != ExecutionStatus::Running
    {
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
    if !may_establish_external_session(&snapshot) {
        return Ok(None);
    }
    let harness_kind = snapshot
        .get("executor_type")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("legacy")
        .to_owned();
    let profile_id = profile_id_for_snapshot_in_tx(transaction, actor_agent_id, &snapshot).await?;
    let capabilities_snapshot_json = snapshot
        .get("harness_capabilities")
        .or_else(|| snapshot.get("capabilities"))
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
    allow_pending: bool,
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
        "SELECT agent_id, harness_kind, status, workspace_id
         FROM harness_session WHERE id = ?",
    )
    .bind(session_id)
    .fetch_optional(&mut **transaction)
    .await?
    .ok_or(DbError::NotFound)?;
    let session_agent_id: String = row.try_get("agent_id")?;
    let session_harness_kind: String = row.try_get("harness_kind")?;
    let status: HarnessSessionStatus = parse_enum(row.try_get::<String, _>("status")?)?;
    let session_workspace_id: Option<String> = row.try_get("workspace_id")?;
    if session_agent_id != *agent_id {
        return Err(DbError::Check(
            "HarnessSession belongs to a different Agent".to_owned(),
        ));
    }
    if let Some(snapshot_json) = input.executor_config_snapshot_json.as_deref() {
        let snapshot =
            serde_json::from_str::<serde_json::Value>(snapshot_json).unwrap_or_default();
        if let Some(executor_type) = snapshot
            .get("executor_type")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            if executor_type != session_harness_kind {
                return Err(DbError::Check(
                    "HarnessSession belongs to a different harness kind".to_owned(),
                ));
            }
        }
    }
    if !matches!(
        &status,
        HarnessSessionStatus::Pending | HarnessSessionStatus::Active
    ) {
        return Err(DbError::Check("HarnessSession is not reusable".to_owned()));
    }
    if matches!(&status, HarnessSessionStatus::Pending) && !allow_pending {
        return Err(DbError::Check(
            "pending HarnessSession cannot be inherited by a new Execution".to_owned(),
        ));
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
               AND status = 'active'
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
        let status = input
            .status
            .clone()
            .unwrap_or_else(|| current.status.clone());
        if matches!(
            &current.status,
            HarnessSessionStatus::Ended | HarnessSessionStatus::Failed
        ) && matches!(
            &status,
            HarnessSessionStatus::Pending | HarnessSessionStatus::Active
        ) {
            return Err(DbError::Check(
                "ended or failed HarnessSession cannot become reusable".to_owned(),
            ));
        }
        if matches!(&status, HarnessSessionStatus::Active)
            && current.external_session_id.is_none()
        {
            return Err(DbError::Check(
                "active HarnessSession requires an external session identity".to_owned(),
            ));
        }
        if matches!(&current.status, HarnessSessionStatus::Active)
            && matches!(&status, HarnessSessionStatus::Pending)
        {
            return Err(DbError::Check(
                "active HarnessSession cannot become pending".to_owned(),
            ));
        }
        let last_activity_at = input.last_activity_at.unwrap_or(current.last_activity_at);
        sqlx::query(
            "UPDATE harness_session
             SET status = ?, last_activity_at = ?, updated_at = ?
             WHERE id = ?",
        )
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
