use super::*;
use crate::{models::*, repository::*};

fn map_work_unit_fallback_receipt(row: SqliteRow) -> Result<WorkUnitFallbackReceipt> {
    Ok(WorkUnitFallbackReceipt {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        work_unit_id: row.try_get("work_unit_id")?,
        work_unit_version_before: row.try_get("work_unit_version_before")?,
        work_unit_version_after: row.try_get("work_unit_version_after")?,
        origin_execution_id: row.try_get("origin_execution_id")?,
        failure_class: row.try_get("failure_class")?,
        route_attempts_digest: row.try_get("route_attempts_digest")?,
        task_role_id: row.try_get("task_role_id")?,
        from_actor_kind: parse_enum(row.try_get("from_actor_kind")?)?,
        from_actor_id: row.try_get("from_actor_id")?,
        to_actor_kind: parse_enum(row.try_get("to_actor_kind")?)?,
        to_actor_id: row.try_get("to_actor_id")?,
        chain_index: row.try_get("chain_index")?,
        cost_class: parse_enum(row.try_get("cost_class")?)?,
        authorization_version: row.try_get("authorization_version")?,
        envelope_digest: row.try_get("envelope_digest")?,
        new_execution_id: row.try_get("new_execution_id")?,
        created_at: row.try_get("created_at")?,
    })
}

fn map_micro_fix_receipt(row: SqliteRow) -> Result<MicroFixReceipt> {
    Ok(MicroFixReceipt {
        id: row.try_get("id")?,
        task_id: row.try_get("task_id")?,
        request_id: row.try_get("request_id")?,
        source_kind: parse_enum(row.try_get("source_kind")?)?,
        finding_refs_json: row.try_get("finding_refs_json")?,
        declared_paths_json: row.try_get("declared_paths_json")?,
        fix_work_unit_id: row.try_get("fix_work_unit_id")?,
        reviewed_commit_sha: row.try_get("reviewed_commit_sha")?,
        result_commit_sha: row.try_get("result_commit_sha")?,
        changed_files: row.try_get("changed_files")?,
        changed_lines: row.try_get("changed_lines")?,
        bounds_outcome: parse_enum(row.try_get("bounds_outcome")?)?,
        bounds_violation: row
            .try_get::<Option<String>, _>("bounds_violation")?
            .map(parse_enum::<MicroFixBoundsViolation>)
            .transpose()?,
        reviewer_cost_class: row
            .try_get::<Option<String>, _>("reviewer_cost_class")?
            .map(parse_enum::<CostClass>)
            .transpose()?,
        reviewer_actor_kind: row
            .try_get::<Option<String>, _>("reviewer_actor_kind")?
            .map(parse_enum::<ActorKind>)
            .transpose()?,
        reviewer_actor_id: row.try_get("reviewer_actor_id")?,
        envelope_digest: row.try_get("envelope_digest")?,
        ordinal: row.try_get("ordinal")?,
        created_at: row.try_get("created_at")?,
        updated_at: row.try_get("updated_at")?,
    })
}

#[async_trait]
impl WorkUnitFallbackReceiptRepo for SqliteDb {
    async fn create_work_unit_fallback_receipt(
        &self,
        input: CreateWorkUnitFallbackReceipt,
    ) -> Result<WorkUnitFallbackReceipt> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO work_unit_fallback_receipt (
                id, task_id, work_unit_id, work_unit_version_before,
                work_unit_version_after, origin_execution_id, failure_class,
                route_attempts_digest, task_role_id, from_actor_kind, from_actor_id,
                to_actor_kind, to_actor_id, chain_index, cost_class,
                authorization_version, envelope_digest, new_execution_id, created_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.work_unit_id)
        .bind(input.work_unit_version_before)
        .bind(input.work_unit_version_after)
        .bind(&input.origin_execution_id)
        .bind(&input.failure_class)
        .bind(&input.route_attempts_digest)
        .bind(&input.task_role_id)
        .bind(input.from_actor_kind.to_string())
        .bind(&input.from_actor_id)
        .bind(input.to_actor_kind.to_string())
        .bind(&input.to_actor_id)
        .bind(input.chain_index)
        .bind(input.cost_class.to_string())
        .bind(input.authorization_version)
        .bind(&input.envelope_digest)
        .bind(&input.new_execution_id)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let row = sqlx::query("SELECT * FROM work_unit_fallback_receipt WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await?;
        let receipt = map_work_unit_fallback_receipt(row)?;
        tx.commit().await?;
        Ok(receipt)
    }

    async fn get_work_unit_fallback_receipt_by_origin_execution(
        &self,
        origin_execution_id: &str,
    ) -> Result<Option<WorkUnitFallbackReceipt>> {
        sqlx::query("SELECT * FROM work_unit_fallback_receipt WHERE origin_execution_id = ?")
            .bind(origin_execution_id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_work_unit_fallback_receipt)
            .transpose()
    }

    async fn list_work_unit_fallback_receipts_by_work_unit(
        &self,
        work_unit_id: &str,
    ) -> Result<Vec<WorkUnitFallbackReceipt>> {
        sqlx::query(
            "SELECT * FROM work_unit_fallback_receipt
             WHERE work_unit_id = ? ORDER BY chain_index ASC, created_at ASC, id ASC",
        )
        .bind(work_unit_id)
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(map_work_unit_fallback_receipt)
        .collect()
    }
}

#[async_trait]
impl MicroFixReceiptRepo for SqliteDb {
    async fn create_micro_fix_receipt(
        &self,
        input: CreateMicroFixReceipt,
    ) -> Result<MicroFixReceipt> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let previous_ordinal: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(ordinal), 0) FROM micro_fix_receipt WHERE task_id = ?",
        )
        .bind(&input.task_id)
        .fetch_one(&mut *tx)
        .await?;
        let ordinal = previous_ordinal + 1;
        if ordinal > MAX_MICRO_FIX_PER_TASK {
            return Err(DbError::Check(
                "micro_fix.max_per_task product limit reached".to_owned(),
            ));
        }

        sqlx::query(
            "INSERT INTO micro_fix_receipt (
                id, task_id, request_id, source_kind, finding_refs_json,
                declared_paths_json, fix_work_unit_id, reviewed_commit_sha,
                bounds_outcome, reviewer_cost_class, reviewer_actor_kind,
                reviewer_actor_id, envelope_digest, ordinal, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'pending', ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&input.id)
        .bind(&input.task_id)
        .bind(&input.request_id)
        .bind(input.source_kind.to_string())
        .bind(&input.finding_refs_json)
        .bind(&input.declared_paths_json)
        .bind(&input.fix_work_unit_id)
        .bind(&input.reviewed_commit_sha)
        .bind(input.reviewer_cost_class.map(|value| value.to_string()))
        .bind(input.reviewer_actor_kind.map(|value| value.to_string()))
        .bind(&input.reviewer_actor_id)
        .bind(&input.envelope_digest)
        .bind(ordinal)
        .bind(&input.created_at)
        .bind(&input.created_at)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        let row = sqlx::query("SELECT * FROM micro_fix_receipt WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await?;
        let receipt = map_micro_fix_receipt(row)?;
        tx.commit().await?;
        Ok(receipt)
    }

    async fn advance_micro_fix_receipt_bounds(
        &self,
        input: AdvanceMicroFixReceiptBounds,
    ) -> Result<MicroFixReceipt> {
        if !matches!(
            input.bounds_outcome,
            MicroFixBoundsOutcome::WithinBounds | MicroFixBoundsOutcome::BoundsExceeded
        ) || (input.bounds_outcome == MicroFixBoundsOutcome::WithinBounds
            && input.bounds_violation.is_some())
            || (input.bounds_outcome == MicroFixBoundsOutcome::BoundsExceeded
                && input.bounds_violation.is_none())
            || input.changed_files < 0
            || input.changed_lines < 0
            || input.result_commit_sha.trim().is_empty()
        {
            return Err(DbError::Check(
                "micro-fix bounds result is incomplete or inconsistent".to_owned(),
            ));
        }

        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let updated = sqlx::query(
            "UPDATE micro_fix_receipt SET
                result_commit_sha = ?, changed_files = ?, changed_lines = ?,
                bounds_outcome = ?, bounds_violation = ?, updated_at = ?
             WHERE id = ? AND bounds_outcome = 'pending'",
        )
        .bind(&input.result_commit_sha)
        .bind(input.changed_files)
        .bind(input.changed_lines)
        .bind(input.bounds_outcome.to_string())
        .bind(input.bounds_violation.map(|value| value.to_string()))
        .bind(&input.updated_at)
        .bind(&input.id)
        .execute(&mut *tx)
        .await
        .map_err(check_error)?;
        if updated.rows_affected() != 1 {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM micro_fix_receipt WHERE id = ?)")
                    .bind(&input.id)
                    .fetch_one(&mut *tx)
                    .await?;
            return Err(if exists {
                DbError::VersionConflict
            } else {
                DbError::NotFound
            });
        }
        let row = sqlx::query("SELECT * FROM micro_fix_receipt WHERE id = ?")
            .bind(&input.id)
            .fetch_one(&mut *tx)
            .await?;
        let receipt = map_micro_fix_receipt(row)?;
        tx.commit().await?;
        Ok(receipt)
    }

    async fn get_micro_fix_receipt(&self, id: &str) -> Result<Option<MicroFixReceipt>> {
        sqlx::query("SELECT * FROM micro_fix_receipt WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .map(map_micro_fix_receipt)
            .transpose()
    }

    async fn list_micro_fix_receipts_by_task(&self, task_id: &str) -> Result<Vec<MicroFixReceipt>> {
        sqlx::query("SELECT * FROM micro_fix_receipt WHERE task_id = ? ORDER BY ordinal ASC")
            .bind(task_id)
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(map_micro_fix_receipt)
            .collect()
    }
}
