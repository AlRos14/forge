use super::*;
use sha2::{Digest, Sha256};

fn validate_object_digest(label: &str, content: &str, digest: &str) -> Result<()> {
    let value: serde_json::Value = serde_json::from_str(content)
        .map_err(|_| DbError::Check(format!("{label} content must be valid JSON")))?;
    if !value.is_object() {
        return Err(DbError::Check(format!(
            "{label} content must be a JSON object"
        )));
    }
    let actual = hex::encode(Sha256::digest(content.as_bytes()));
    if actual != digest {
        return Err(DbError::Check(format!(
            "{label} digest does not match its content"
        )));
    }
    Ok(())
}

fn map_validation_run(row: SqliteRow) -> Result<ValidationRun> {
    Ok(ValidationRun {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        caused_by_execution_id: row.try_get("caused_by_execution_id")?,
        check_identity: row.try_get("check_identity")?,
        command: row.try_get("command")?,
        config_summary_json: row.try_get("config_summary_json")?,
        config_digest: row.try_get("config_digest")?,
        workspace_id: row.try_get("workspace_id")?,
        commit_sha: row.try_get("commit_sha")?,
        workspace_snapshot_digest: row.try_get("workspace_snapshot_digest")?,
        idempotency_key: row.try_get("idempotency_key")?,
        status: parse_enum(row.try_get("status")?)?,
        exit_code: row.try_get("exit_code")?,
        started_at: row.try_get("started_at")?,
        finished_at: row.try_get("finished_at")?,
        logs_ref: row.try_get("logs_ref")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

fn map_evidence(row: SqliteRow) -> Result<Evidence> {
    Ok(Evidence {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        kind: row.try_get("kind")?,
        content_json: row.try_get("content_json")?,
        digest: row.try_get("digest")?,
        producer_validation_run_id: row.try_get("validation_run_id")?,
        evidence_key: row.try_get("evidence_key")?,
        created_at: row.try_get("created_at")?,
    })
}

async fn get_validation_run_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    id: &str,
) -> Result<Option<ValidationRun>> {
    sqlx::query("SELECT * FROM validation_run WHERE id = ?")
        .bind(id)
        .fetch_optional(&mut **tx)
        .await?
        .map(map_validation_run)
        .transpose()
}

async fn get_evidence_for_run_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    run_id: &str,
) -> Result<Vec<Evidence>> {
    let rows = sqlx::query(
        "SELECT e.*, p.validation_run_id, p.evidence_key
         FROM evidence e JOIN evidence_validation_run_producer p ON p.evidence_id = e.id
         WHERE p.validation_run_id = ? ORDER BY p.evidence_key ASC, e.id ASC",
    )
    .bind(run_id)
    .fetch_all(&mut **tx)
    .await?;
    rows.into_iter().map(map_evidence).collect()
}

fn same_validation_run_identity(left: &ValidationRun, right: &CreateValidationRun) -> bool {
    left.task_id == right.task_id
        && left.work_unit_id == right.work_unit_id
        && left.caused_by_execution_id == right.caused_by_execution_id
        && left.check_identity == right.check_identity
        && left.command == right.command
        && left.config_summary_json == right.config_summary_json
        && left.config_digest == right.config_digest
        && left.workspace_id == right.workspace_id
        && left.commit_sha == right.commit_sha
        && left.workspace_snapshot_digest == right.workspace_snapshot_digest
        && left.idempotency_key == right.idempotency_key
}

fn same_evidence(existing: &[Evidence], proposed: &[CreateEvidence]) -> bool {
    existing.len() == proposed.len()
        && proposed.iter().all(|item| {
            existing.iter().any(|row| {
                row.id == item.id
                    && row.task_id == item.task_id
                    && row.kind == item.kind
                    && row.content_json == item.content_json
                    && row.digest == item.digest
                    && row.producer_validation_run_id == item.validation_run_id
                    && row.evidence_key == item.evidence_key
            })
        })
}

#[async_trait]
impl ValidationRunRepo for SqliteDb {
    async fn start_validation_run(
        &self,
        input: CreateValidationRun,
        event: CreateDomainEvent,
    ) -> Result<ValidationRunStartWrite> {
        validate_object_digest(
            "ValidationRun configuration summary",
            &input.config_summary_json,
            &input.config_digest,
        )?;
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query(
            "INSERT INTO validation_run (
                id, task_id, work_unit_id, caused_by_execution_id, check_identity, command,
                config_summary_json, config_digest, workspace_id, commit_sha,
                workspace_snapshot_digest, idempotency_key, status, started_at, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'running', ?, ?, ?)
             ON CONFLICT(idempotency_key) DO NOTHING",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.work_unit_id)
        .bind(&input.caused_by_execution_id)
        .bind(&input.check_identity)
        .bind(&input.command)
        .bind(&input.config_summary_json)
        .bind(&input.config_digest)
        .bind(&input.workspace_id)
        .bind(&input.commit_sha)
        .bind(&input.workspace_snapshot_digest)
        .bind(&input.idempotency_key)
        .bind(&input.started_at)
        .bind(&input.created_at)
        .bind(&input.updated_at)
        .execute(&mut *tx)
        .await?
        .rows_affected();

        let record = sqlx::query("SELECT * FROM validation_run WHERE idempotency_key = ?")
            .bind(&input.idempotency_key)
            .fetch_optional(&mut *tx)
            .await?
            .map(map_validation_run)
            .transpose()?
            .ok_or(DbError::NotFound)?;
        if !same_validation_run_identity(&record, &input) {
            return Err(DbError::Check(
                "ValidationRun idempotency key is already bound to a different exact subject"
                    .to_owned(),
            ));
        }

        let event = if inserted == 1 {
            Some(DomainEventRepo::append_event_in_tx(self, &mut tx, &event).await?)
        } else {
            None
        };
        tx.commit().await?;
        Ok(ValidationRunStartWrite {
            validation_run: record,
            event,
        })
    }

    async fn claim_validation_run(
        &self,
        id: &str,
        owner: &str,
        now: &str,
        claim_until: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE validation_run SET claim_owner = ?, claim_until = ?, updated_at = ?
             WHERE id = ? AND status = 'running'
               AND (claim_until IS NULL OR claim_until < ? OR claim_owner = ?)",
        )
        .bind(owner)
        .bind(claim_until)
        .bind(now)
        .bind(id)
        .bind(now)
        .bind(owner)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn heartbeat_validation_run(
        &self,
        id: &str,
        owner: &str,
        now: &str,
        claim_until: &str,
    ) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE validation_run SET claim_until = ?, updated_at = ?
             WHERE id = ? AND status = 'running' AND claim_owner = ? AND claim_until >= ?",
        )
        .bind(claim_until)
        .bind(now)
        .bind(id)
        .bind(owner)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    async fn finish_validation_run(
        &self,
        input: FinishValidationRun,
    ) -> Result<ValidationRunCompletionWrite> {
        if input.status == ValidationRunStatus::Running {
            return Err(DbError::Check(
                "ValidationRun finish status cannot be running".into(),
            ));
        }
        for evidence in &input.evidence {
            validate_object_digest("Evidence", &evidence.content_json, &evidence.digest)?;
        }
        if let Some(report) = &input.validation_report {
            validate_object_digest("ValidationReport", &report.content, &report.digest)?;
        }
        let mut tx = self.pool.begin().await?;
        let current = get_validation_run_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;

        if current.status != ValidationRunStatus::Running {
            let existing = get_evidence_for_run_in_tx(&mut tx, &input.id).await?;
            let existing_report = sqlx::query(
                "SELECT a.id, a.task_id, a.kind, a.storage_kind, a.content,
                        a.content_ref, a.metadata_json, a.digest,
                        p.validation_run_id, p.task_id AS producer_task_id
                 FROM artifact_validation_run_producer p
                 JOIN artifact a ON a.id = p.artifact_id
                 WHERE p.validation_run_id = ?",
            )
            .bind(&input.id)
            .fetch_optional(&mut *tx)
            .await?;
            let report_matches = match (&input.validation_report, existing_report.as_ref()) {
                (None, None) => true,
                (Some(proposed), Some(row)) => {
                    row.try_get::<String, _>("task_id")? == proposed.task_id
                        && row.try_get::<String, _>("producer_task_id")? == proposed.task_id
                        && row.try_get::<String, _>("kind")? == "validation_report"
                        && row.try_get::<String, _>("storage_kind")? == "inline"
                        && row.try_get::<Option<String>, _>("content")?.as_deref()
                            == Some(proposed.content.as_str())
                        && row.try_get::<Option<String>, _>("content_ref")?.is_none()
                        && row.try_get::<String, _>("metadata_json")? == proposed.metadata_json
                        && row.try_get::<Option<String>, _>("digest")?.as_deref()
                            == Some(proposed.digest.as_str())
                        && row.try_get::<String, _>("validation_run_id")? == input.id
                }
                _ => false,
            };
            if current.status != input.status
                || current.exit_code != input.exit_code
                || current.logs_ref.as_deref() != Some(input.logs_ref.as_str())
                || !same_evidence(&existing, &input.evidence)
                || !report_matches
            {
                return Err(DbError::Check(
                    "ValidationRun already completed with a conflicting result".to_owned(),
                ));
            }
            let report_id = existing_report
                .as_ref()
                .map(|row| row.try_get::<String, _>("id"))
                .transpose()?;
            tx.rollback().await?;
            return Ok(ValidationRunCompletionWrite {
                validation_run: current,
                evidence: existing,
                validation_report: match report_id {
                    Some(id) => super::collaboration::get_artifact_row(self, &id).await?,
                    None => None,
                },
                events: Vec::new(),
            });
        }
        if current.id != input.id {
            return Err(DbError::Check("ValidationRun identity mismatch".into()));
        }
        let owns = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM validation_run
             WHERE id = ? AND status = 'running' AND claim_owner = ? AND claim_until >= ?",
        )
        .bind(&input.id)
        .bind(&input.claim_owner)
        .bind(&input.finished_at)
        .fetch_one(&mut *tx)
        .await?;
        if owns != 1 {
            return Err(DbError::VersionConflict);
        }

        for item in &input.evidence {
            if item.validation_run_id != input.id || item.task_id != current.task_id {
                return Err(DbError::Check(
                    "Evidence must reference the exact same-Task ValidationRun".to_owned(),
                ));
            }
            sqlx::query(
                "INSERT INTO evidence_validation_run_producer
                 (evidence_id, validation_run_id, task_id, evidence_key)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(&item.id)
            .bind(&item.validation_run_id)
            .bind(&item.task_id)
            .bind(&item.evidence_key)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO evidence (id, task_id, kind, content_json, digest, created_at)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(&item.id)
            .bind(&item.task_id)
            .bind(&item.kind)
            .bind(&item.content_json)
            .bind(&item.digest)
            .bind(&item.created_at)
            .execute(&mut *tx)
            .await?;
        }

        let validation_report = if let Some(report) = &input.validation_report {
            if report.validation_run_id != input.id || report.task_id != current.task_id {
                return Err(DbError::Check(
                    "ValidationReport must reference the exact same-Task ValidationRun".into(),
                ));
            }
            sqlx::query(
                "INSERT INTO artifact_validation_run_producer
                 (artifact_id, validation_run_id, task_id) VALUES (?, ?, ?)",
            )
            .bind(&report.id)
            .bind(&report.validation_run_id)
            .bind(&report.task_id)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO validation_run_artifact_output
                 (validation_run_id, artifact_id, task_id, kind, digest, created_at)
                 VALUES (?, ?, ?, 'validation_report', ?, ?)",
            )
            .bind(&report.validation_run_id)
            .bind(&report.id)
            .bind(&report.task_id)
            .bind(&report.digest)
            .bind(&report.created_at)
            .execute(&mut *tx)
            .await?;
            sqlx::query(
                "INSERT INTO artifact (
                    id, task_id, kind, storage_kind, content, content_ref,
                    metadata_json, digest, created_at
                 ) VALUES (?, ?, 'validation_report', 'inline', ?, NULL, ?, ?, ?)",
            )
            .bind(&report.id)
            .bind(&report.task_id)
            .bind(&report.content)
            .bind(&report.metadata_json)
            .bind(&report.digest)
            .bind(&report.created_at)
            .execute(&mut *tx)
            .await?;
            Some(report.id.clone())
        } else {
            None
        };

        let mut events = Vec::with_capacity(input.events.len());
        for event in &input.events {
            events.push(DomainEventRepo::append_event_in_tx(self, &mut tx, event).await?);
        }
        let result = sqlx::query(
            "UPDATE validation_run SET status = ?, exit_code = ?, finished_at = ?, logs_ref = ?,
                 claim_owner = NULL, claim_until = NULL, updated_at = ?
             WHERE id = ? AND status = 'running' AND claim_owner = ? AND claim_until >= ?",
        )
        .bind(input.status.to_string())
        .bind(input.exit_code)
        .bind(&input.finished_at)
        .bind(&input.logs_ref)
        .bind(&input.finished_at)
        .bind(&input.id)
        .bind(&input.claim_owner)
        .bind(&input.finished_at)
        .execute(&mut *tx)
        .await?;
        if result.rows_affected() != 1 {
            return Err(DbError::VersionConflict);
        }

        let record = get_validation_run_in_tx(&mut tx, &input.id)
            .await?
            .ok_or(DbError::NotFound)?;
        let evidence = get_evidence_for_run_in_tx(&mut tx, &input.id).await?;
        tx.commit().await?;
        let validation_report = if let Some(artifact_id) = validation_report {
            super::collaboration::get_artifact_row(self, &artifact_id).await?
        } else {
            None
        };
        Ok(ValidationRunCompletionWrite {
            validation_run: record,
            evidence,
            validation_report,
            events,
        })
    }

    async fn get_validation_run(&self, id: &str) -> Result<Option<ValidationRun>> {
        sqlx::query("SELECT * FROM validation_run WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_run)
            .transpose()
    }

    async fn get_validation_run_by_idempotency_key(
        &self,
        idempotency_key: &str,
    ) -> Result<Option<ValidationRun>> {
        sqlx::query("SELECT * FROM validation_run WHERE idempotency_key = ?")
            .bind(idempotency_key)
            .fetch_optional(&self.pool)
            .await?
            .map(map_validation_run)
            .transpose()
    }

    async fn list_validation_runs_by_task(&self, task_id: &str) -> Result<Vec<ValidationRun>> {
        let rows = sqlx::query(
            "SELECT * FROM validation_run WHERE task_id = ? ORDER BY created_at ASC, id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_validation_run).collect()
    }

    async fn list_validation_runs_for_subject(
        &self,
        task_id: &str,
        workspace_id: &str,
        commit_sha: &str,
        workspace_snapshot_digest: &str,
        caused_by_execution_id: Option<&str>,
    ) -> Result<Vec<ValidationRun>> {
        let rows = sqlx::query(
            "SELECT * FROM validation_run WHERE task_id = ? AND workspace_id = ?
               AND commit_sha = ? AND workspace_snapshot_digest = ?
               AND caused_by_execution_id IS ?
             ORDER BY check_identity ASC, created_at ASC, id ASC",
        )
        .bind(task_id)
        .bind(workspace_id)
        .bind(commit_sha)
        .bind(workspace_snapshot_digest)
        .bind(caused_by_execution_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_validation_run).collect()
    }

    async fn get_evidence(&self, id: &str) -> Result<Option<Evidence>> {
        sqlx::query(
            "SELECT e.*, p.validation_run_id, p.evidence_key
             FROM evidence e JOIN evidence_validation_run_producer p ON p.evidence_id = e.id
             WHERE e.id = ?",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?
        .map(map_evidence)
        .transpose()
    }

    async fn list_evidence_by_task(&self, task_id: &str) -> Result<Vec<Evidence>> {
        let rows = sqlx::query(
            "SELECT e.*, p.validation_run_id, p.evidence_key
             FROM evidence e JOIN evidence_validation_run_producer p ON p.evidence_id = e.id
             WHERE e.task_id = ? ORDER BY e.created_at ASC, e.id ASC",
        )
        .bind(task_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter().map(map_evidence).collect()
    }

    async fn list_evidence_for_validation_run(
        &self,
        validation_run_id: &str,
    ) -> Result<Vec<Evidence>> {
        let mut tx = self.pool.begin().await?;
        let rows = get_evidence_for_run_in_tx(&mut tx, validation_run_id).await?;
        tx.commit().await?;
        Ok(rows)
    }

    async fn list_execution_evidence_inputs(
        &self,
        execution_id: &str,
    ) -> Result<Vec<ExecutionEvidenceInput>> {
        let rows = sqlx::query(
            "SELECT execution_id, evidence_id, task_id, digest, created_at
             FROM execution_evidence_input WHERE execution_id = ?
             ORDER BY created_at ASC, evidence_id ASC",
        )
        .bind(execution_id)
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ExecutionEvidenceInput {
                    execution_id: row.try_get("execution_id")?,
                    evidence_id: row.try_get("evidence_id")?,
                    task_id: row.try_get("task_id")?,
                    digest: row.try_get("digest")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }

    async fn get_validation_run_artifact_output(
        &self,
        validation_run_id: &str,
    ) -> Result<Option<Artifact>> {
        let artifact_id = sqlx::query_scalar::<_, String>(
            "SELECT artifact_id FROM validation_run_artifact_output
             WHERE validation_run_id = ?",
        )
        .bind(validation_run_id)
        .fetch_optional(self.pool())
        .await?;
        match artifact_id {
            Some(artifact_id) => super::collaboration::get_artifact_row(self, &artifact_id).await,
            None => Ok(None),
        }
    }

    async fn pin_execution_evidence_inputs(
        &self,
        execution_id: &str,
        evidence_ids: &[String],
        created_at: &str,
    ) -> Result<Vec<ExecutionEvidenceInput>> {
        let mut tx = self.pool.begin().await?;
        let mut result = Vec::with_capacity(evidence_ids.len());
        let mut seen = Vec::<&str>::new();
        for evidence_id in evidence_ids {
            if seen.contains(&evidence_id.as_str()) {
                continue;
            }
            seen.push(evidence_id);
            let row = sqlx::query("SELECT task_id, digest FROM evidence WHERE id = ?")
                .bind(evidence_id)
                .fetch_optional(&mut *tx)
                .await?
                .ok_or(DbError::NotFound)?;
            let task_id: String = row.try_get("task_id")?;
            let digest: String = row.try_get("digest")?;
            sqlx::query(
                "INSERT INTO execution_evidence_input
                 (execution_id, evidence_id, task_id, digest, created_at)
                 VALUES (?, ?, ?, ?, ?) ON CONFLICT(execution_id, evidence_id) DO NOTHING",
            )
            .bind(execution_id)
            .bind(evidence_id)
            .bind(&task_id)
            .bind(&digest)
            .bind(created_at)
            .execute(&mut *tx)
            .await?;
            let record = sqlx::query(
                "SELECT execution_id, evidence_id, task_id, digest, created_at
                 FROM execution_evidence_input WHERE execution_id = ? AND evidence_id = ?",
            )
            .bind(execution_id)
            .bind(evidence_id)
            .fetch_one(&mut *tx)
            .await?;
            result.push(ExecutionEvidenceInput {
                execution_id: record.try_get("execution_id")?,
                evidence_id: record.try_get("evidence_id")?,
                task_id: record.try_get("task_id")?,
                digest: record.try_get("digest")?,
                created_at: record.try_get("created_at")?,
            });
        }
        tx.commit().await?;
        Ok(result)
    }
}
