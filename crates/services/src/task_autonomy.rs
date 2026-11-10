use std::sync::Arc;

use api_types::{TaskAutonomyLimits, UpdateTaskAutonomyLimitsRequest};
use db::{now_rfc3339, DomainEventRepo, SqliteDb};
use events::EventBus;
use sha2::{Digest, Sha256};
use sqlx::Row;

use crate::{
    collaboration_service::{CollaborationActorSource, CollaborationService},
    Result, ServiceError,
};

pub struct TaskAutonomyService {
    db: Arc<SqliteDb>,
    events: Arc<EventBus>,
}

impl TaskAutonomyService {
    pub fn new(db: Arc<SqliteDb>, events: Arc<EventBus>) -> Self {
        Self { db, events }
    }

    pub async fn get(&self, task_id: &str, user_id: &str) -> Result<TaskAutonomyLimits> {
        CollaborationService::new(self.db.clone(), self.events.clone())
            .authorize_source(
                task_id,
                &CollaborationActorSource::Human(user_id.to_owned()),
            )
            .await?;
        let row = sqlx::query("SELECT l.*, c.max_turns, c.max_quality_requests FROM task_autonomy_limits l JOIN task_coordination c ON c.task_id=l.task_id WHERE l.task_id=?")
            .bind(task_id).fetch_one(self.db.pool()).await?;
        Ok(TaskAutonomyLimits {
            version: row.try_get("version")?,
            max_turns: row.try_get("max_turns")?,
            max_quality_requests: row.try_get("max_quality_requests")?,
            max_rework_rounds: row.try_get("max_rework_rounds")?,
            max_route_requests: row.try_get("max_route_requests")?,
        })
    }

    pub async fn update(
        &self,
        task_id: &str,
        user_id: &str,
        input: UpdateTaskAutonomyLimitsRequest,
    ) -> Result<TaskAutonomyLimits> {
        if input.limits.version != input.expected_version {
            return Err(ServiceError::conflict(
                "Limits version must match expected_version",
            ));
        }
        for value in [
            input.limits.max_turns,
            input.limits.max_quality_requests,
            input.limits.max_rework_rounds,
            input.limits.max_route_requests,
        ] {
            if !(1..=1000).contains(&value) {
                return Err(ServiceError::invalid_operation(
                    "Autonomy limits must be between 1 and 1000",
                ));
            }
        }
        if input.idempotency_key.trim().is_empty()
            || input.idempotency_key.len() > 256
            || input.reason.trim().is_empty()
            || input.reason.len() > 4096
        {
            return Err(ServiceError::invalid_operation(
                "A bounded idempotency key and reason are required",
            ));
        }
        let digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&(user_id, &input))
                .map_err(|e| ServiceError::invalid_operation(e.to_string()))?,
        ));
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        // Limits authorize additional provider work: require a current owner or
        // administrator inside the same write transaction, including replay.
        let authorized: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task t JOIN project_member m ON m.project_id=t.project_id JOIN user u ON u.id=m.user_id WHERE t.id=? AND t.deleted_at IS NULL AND m.user_id=? AND m.role IN ('owner','admin'))")
            .bind(task_id).bind(user_id).fetch_one(&mut *tx).await?;
        if !authorized {
            return Err(ServiceError::AuthorizationDenied {
                message: "Only a current Project owner or administrator may change autonomy limits"
                    .to_owned(),
            });
        }
        if let Some(row)=sqlx::query("SELECT request_digest,response_json FROM task_autonomy_limit_receipt WHERE task_id=? AND idempotency_key=?")
            .bind(task_id).bind(&input.idempotency_key).fetch_optional(&mut *tx).await? {
            if row.try_get::<String,_>("request_digest")? != digest { return Err(db::DbError::IdempotencyConflict.into()); }
            let result=serde_json::from_str(&row.try_get::<String,_>("response_json")?).map_err(|e|ServiceError::invalid_operation(e.to_string()))?;
            tx.commit().await?;
            return Ok(result);
        }
        let now = now_rfc3339();
        let updated=sqlx::query("UPDATE task_autonomy_limits SET max_rework_rounds=?, max_route_requests=?, version=version+1, updated_at=? WHERE task_id=? AND version=?")
            .bind(input.limits.max_rework_rounds).bind(input.limits.max_route_requests).bind(&now).bind(task_id).bind(input.expected_version).execute(&mut *tx).await?;
        if updated.rows_affected() != 1 {
            return Err(db::DbError::VersionConflict.into());
        }
        let used_route: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(budget_used),0) FROM task_quality_route WHERE task_id=?",
        )
        .bind(task_id)
        .fetch_one(&mut *tx)
        .await?;
        if input.limits.max_route_requests < used_route {
            return Err(ServiceError::conflict(
                "A route limit cannot be lower than its consumed rounds",
            ));
        }
        sqlx::query("UPDATE task_coordination SET max_turns=?,max_quality_requests=?,updated_at=? WHERE task_id=?")
            .bind(input.limits.max_turns).bind(input.limits.max_quality_requests).bind(&now).bind(task_id).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_quality_route SET max_requests=?,updated_at=? WHERE task_id=?")
            .bind(input.limits.max_route_requests)
            .bind(&now)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        let result = TaskAutonomyLimits {
            version: input.expected_version + 1,
            ..input.limits
        };
        let response = serde_json::to_string(&result)
            .map_err(|e| ServiceError::invalid_operation(e.to_string()))?;
        sqlx::query("INSERT INTO task_autonomy_limit_receipt(task_id,idempotency_key,request_digest,response_json,created_at) VALUES(?,?,?,?,?)")
            .bind(task_id).bind(&input.idempotency_key).bind(&digest).bind(&response).bind(&now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO task_autonomy_retry_grant(intent_id,limits_version,user_id,created_at)
            SELECT id,?,?,? FROM task_rework_automation_intent WHERE task_id=? AND state='blocked' AND block_reason LIKE '%rework budget exhausted%'
            AND (SELECT count(*) FROM task_rework_receipt WHERE task_id=?) + (SELECT count(*) FROM task_review_triage WHERE task_id=? AND disposition='recheck') < ?")
            .bind(result.version).bind(user_id).bind(&now).bind(task_id).bind(task_id).bind(task_id).bind(result.max_rework_rounds).execute(&mut *tx).await?;
        sqlx::query("UPDATE task_rework_automation_intent SET state='pending',block_reason=NULL,updated_at=? WHERE task_id=? AND state='blocked' AND EXISTS(SELECT 1 FROM task_autonomy_retry_grant grant_record WHERE grant_record.intent_id=task_rework_automation_intent.id AND grant_record.limits_version=?)")
            .bind(&now).bind(task_id).bind(result.version).execute(&mut *tx).await?;
        let exhausted=sqlx::query("SELECT id,route_id FROM task_quality_request WHERE task_id=? AND state='requires_human' AND requires_human_reason_code IN ('coordination.budget_exhausted','coordination.quality_route_budget_exhausted') ORDER BY created_at,id")
            .bind(task_id).fetch_all(&mut *tx).await?;
        for request in exhausted {
            let route_id: String = request.try_get("route_id")?;
            let global_available:bool=sqlx::query_scalar("SELECT quality_request_count<max_quality_requests FROM task_coordination WHERE task_id=?").bind(task_id).fetch_one(&mut *tx).await?;
            let route_available:bool=sqlx::query_scalar("SELECT budget_used<max_requests FROM task_quality_route WHERE task_id=? AND route_id=?").bind(task_id).bind(&route_id).fetch_one(&mut *tx).await?;
            if !global_available || !route_available {
                continue;
            }
            let request_id: String = request.try_get("id")?;
            sqlx::query("INSERT INTO task_quality_retry_grant(request_id,limits_version,user_id,created_at) VALUES(?,?,?,?)").bind(&request_id).bind(result.version).bind(user_id).bind(&now).execute(&mut *tx).await?;
            sqlx::query("UPDATE task_quality_route SET budget_used=budget_used+1,updated_at=? WHERE task_id=? AND route_id=?").bind(&now).bind(task_id).bind(&route_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE task_coordination SET quality_request_count=quality_request_count+1 WHERE task_id=?").bind(task_id).execute(&mut *tx).await?;
            sqlx::query("UPDATE task_quality_request SET state='requested',requires_human_reason_code=NULL,requires_human_reason=NULL,updated_at=?,version=version+1 WHERE id=?").bind(&now).bind(&request_id).execute(&mut *tx).await?;
        }
        let event = DomainEventRepo::append_event_in_tx(
            &*self.db,
            &mut tx,
            &db::CreateDomainEvent {
                id: db::new_uuid_v4(),
                event_type: "task.autonomy_limits_changed".to_owned(),
                entity_type: "task".to_owned(),
                entity_id: task_id.to_owned(),
                actor_type: "human".to_owned(),
                actor_id: Some(user_id.to_owned()),
                scope_type: "task".to_owned(),
                scope_id: task_id.to_owned(),
                correlation_id: task_id.to_owned(),
                causation_id: None,
                causation_depth: 0,
                dedupe_key: None,
                payload_json:
                    serde_json::json!({"task_id":task_id,"limits":result,"reason":input.reason})
                        .to_string(),
                created_at: now,
            },
        )
        .await?;
        tx.commit().await?;
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.events,
            &event,
        );
        Ok(result)
    }
}
