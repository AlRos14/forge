use super::*;

#[async_trait]
impl DaemonRepo for SqliteDb {
    async fn upsert_by_machine_id(&self, input: UpsertDaemon) -> Result<Daemon> {
        let changed = sqlx::query("INSERT INTO daemon (id, machine_id, hostname, os, arch, agent_version, labels_json, status, registration_token_hash, owner_id, visibility, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(machine_id) DO UPDATE SET hostname = excluded.hostname, os = excluded.os, arch = excluded.arch, agent_version = excluded.agent_version, labels_json = excluded.labels_json, status = excluded.status, registration_token_hash = excluded.registration_token_hash, owner_id = excluded.owner_id, visibility = excluded.visibility, updated_at = excluded.updated_at WHERE daemon.owner_id IS excluded.owner_id")
            .bind(&input.id)
            .bind(&input.machine_id)
            .bind(&input.hostname)
            .bind(&input.os)
            .bind(&input.arch)
            .bind(input.agent_version.as_deref())
            .bind(&input.labels_json)
            .bind(input.status.to_string())
            .bind(input.registration_token_hash.as_deref())
            .bind(input.owner_id.as_deref())
            .bind(&input.visibility)
            .bind(&input.created_at)
            .bind(&input.updated_at)
            .execute(&self.pool)
            .await?;
        if changed.rows_affected() != 1 {
            return Err(DbError::Check(
                "daemon registration belongs to another account".to_owned(),
            ));
        }
        sqlx::query("SELECT * FROM daemon WHERE machine_id = ?")
            .bind(&input.machine_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_daemon)
            .transpose()?
            .ok_or(DbError::NotFound)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Daemon>> {
        sqlx::query("SELECT * FROM daemon WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_daemon)
            .transpose()
    }

    async fn get_by_machine_id(&self, machine_id: &str) -> Result<Option<Daemon>> {
        sqlx::query("SELECT * FROM daemon WHERE machine_id = ?")
            .bind(machine_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_daemon)
            .transpose()
    }

    async fn list(&self, page: PageRequest) -> Result<Page<Daemon>> {
        let offset = decode_offset(&page.cursor)?;
        let sql = format!(
            "SELECT * FROM daemon ORDER BY {} LIMIT ? OFFSET ?",
            order_clause_without_priority(&page)
        );
        let rows = sqlx::query(&sql)
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_daemon)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            total_count(&self.pool, "SELECT COUNT(*) FROM daemon").await?
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn list_visible(&self, user_id: Option<&str>, page: PageRequest) -> Result<Page<Daemon>> {
        let offset = decode_offset(&page.cursor)?;
        let (sql, total_sql, has_user_param) = match user_id {
            Some(_) => (
                format!(
                    "SELECT * FROM daemon WHERE visibility = 'global' OR owner_id IS NULL OR owner_id = ? ORDER BY {} LIMIT ? OFFSET ?",
                    order_clause_without_priority(&page)
                ),
                "SELECT COUNT(*) FROM daemon WHERE visibility = 'global' OR owner_id IS NULL OR owner_id = ?"
                    as &str,
                true,
            ),
            None => (
                format!(
                    "SELECT * FROM daemon WHERE visibility = 'global' OR owner_id IS NULL ORDER BY {} LIMIT ? OFFSET ?",
                    order_clause_without_priority(&page)
                ),
                "SELECT COUNT(*) FROM daemon WHERE visibility = 'global' OR owner_id IS NULL"
                    as &str,
                false,
            ),
        };
        let mut query = sqlx::query(&sql);
        if let Some(uid) = user_id {
            query = query.bind(uid);
        }
        let rows = query
            .bind(limit(&page) + 1)
            .bind(offset)
            .fetch_all(&self.pool)
            .await?;
        let items = rows
            .into_iter()
            .map(map_daemon)
            .collect::<Result<Vec<_>>>()?;
        let total = if page.include_total {
            let mut tc = sqlx::query_scalar::<_, i64>(total_sql);
            if has_user_param {
                tc = tc.bind(user_id.unwrap());
            }
            Some(tc.fetch_one(&self.pool).await?)
        } else {
            None
        };
        page_from_items(items, &page, offset, total)
    }

    async fn get_visible(&self, id: &str, user_id: Option<&str>) -> Result<Option<Daemon>> {
        let daemon = DaemonRepo::get_by_id(self, id).await?;
        match daemon {
            Some(d) => {
                if d.visibility == "global" || d.owner_id.is_none() {
                    return Ok(Some(d));
                }
                if let Some(uid) = user_id {
                    if d.owner_id.as_deref() == Some(uid) {
                        return Ok(Some(d));
                    }
                }
                Ok(None)
            }
            None => Ok(None),
        }
    }

    async fn update_report(&self, input: UpdateDaemonReport) -> Result<Daemon> {
        let result = match &input.labels_json {
            Some(labels_json) => {
                sqlx::query("UPDATE daemon SET last_report_at = ?, status = ?, detected_clis_json = ?, labels_json = ?, updated_at = ?, version = version + 1 WHERE id = ?")
                    .bind(&input.last_report_at)
                    .bind(input.status.to_string())
                    .bind(&input.detected_clis_json)
                    .bind(labels_json)
                    .bind(&input.updated_at)
                    .bind(&input.id)
                    .execute(&self.pool)
                    .await?
            }
            None => {
                sqlx::query("UPDATE daemon SET last_report_at = ?, status = ?, detected_clis_json = ?, updated_at = ?, version = version + 1 WHERE id = ?")
                    .bind(&input.last_report_at)
                    .bind(input.status.to_string())
                    .bind(&input.detected_clis_json)
                    .bind(&input.updated_at)
                    .bind(&input.id)
                    .execute(&self.pool)
                    .await?
            }
        };
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        DaemonRepo::get_by_id(self, &input.id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn mark_online(&self, id: &str, last_report_at: &str) -> Result<Daemon> {
        let result = sqlx::query(
            "UPDATE daemon SET status = 'online', last_report_at = ?, updated_at = ?, version = version + 1 WHERE id = ?",
        )
        .bind(last_report_at)
        .bind(last_report_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        DaemonRepo::get_by_id(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn mark_offline(&self, id: &str, updated_at: &str) -> Result<Daemon> {
        let result = sqlx::query(
            "UPDATE daemon SET status = 'offline', updated_at = ?, version = version + 1 WHERE id = ?",
        )
        .bind(updated_at)
        .bind(id)
        .execute(&self.pool)
        .await?;
        if result.rows_affected() == 0 {
            return Err(DbError::NotFound);
        }
        DaemonRepo::get_by_id(self, id)
            .await?
            .ok_or(DbError::NotFound)
    }

    async fn list_available_for_executor(
        &self,
        executor_type: &str,
        owner_id: &str,
    ) -> Result<Vec<Daemon>> {
        let Some(owner_is_admin) =
            sqlx::query_scalar::<_, bool>("SELECT is_admin FROM user WHERE id = ?")
                .bind(owner_id)
                .fetch_optional(&self.pool)
                .await?
        else {
            return Ok(Vec::new());
        };
        let rows = sqlx::query(
            "SELECT * FROM daemon WHERE status = 'online' ORDER BY created_at ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        let mut daemons = Vec::new();
        for row in rows {
            let daemon = map_daemon(row)?;
            if crate::daemon_execution_authorized(
                &daemon,
                Some(owner_id),
                owner_is_admin,
                executor_type,
            ) {
                daemons.push(daemon);
            }
        }
        Ok(daemons)
    }
}

/// Validate the immutable route against current account/profile/host authority.
/// Used at admission and at the serialized one-attempt Provider Start claim.
pub(super) async fn execution_daemon_authorized_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    agent_id: &str,
    snapshot_json: Option<&str>,
) -> Result<bool> {
    let Some(snapshot) =
        snapshot_json.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    else {
        return Ok(false);
    };
    let Some(daemon_id) = snapshot.get("resolved_daemon_id").and_then(|v| v.as_str()) else {
        return Ok(false);
    };
    let agent = sqlx::query("SELECT * FROM agent_current WHERE id = ?")
        .bind(agent_id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_agent)
        .transpose()?;
    let daemon = sqlx::query("SELECT * FROM daemon WHERE id = ?")
        .bind(daemon_id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_daemon)
        .transpose()?;
    let (Some(agent), Some(daemon)) = (agent, daemon) else {
        return Ok(false);
    };
    let Some(owner_id) = agent.owner_id.as_deref() else {
        return Ok(false);
    };
    let Some(is_admin) = sqlx::query_scalar::<_, bool>("SELECT is_admin FROM user WHERE id = ?")
        .bind(owner_id)
        .fetch_optional(&mut **tx)
        .await?
    else {
        return Ok(false);
    };
    Ok(!agent.paused
        && agent.backend_kind != "native"
        && snapshot.get("agent_id").and_then(|v| v.as_str()) == Some(agent_id)
        && snapshot.get("daemon_authority")
            == Some(&crate::daemon_authority_snapshot(&agent, &daemon))
        && crate::daemon_execution_authorized(
            &daemon,
            Some(owner_id),
            is_admin,
            &agent.executor_type,
        ))
}
