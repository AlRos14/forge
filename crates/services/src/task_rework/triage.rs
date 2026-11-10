use super::*;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewFindingsDecision {
    pub authorization_version: i64,
    pub review_report_artifact_id: String,
    pub report_digest: String,
    pub disposition: ReviewFindingsDisposition,
    pub rationale: String,
    pub instructions: String,
}

use api_types::TaskReviewDisposition as ReviewFindingsDisposition;

impl TaskReworkService {
    pub async fn list_review_triage(
        &self,
        task_id: &str,
    ) -> Result<Vec<api_types::TaskReviewTriageResponse>> {
        let rows=sqlx::query("SELECT * FROM task_review_triage WHERE task_id=? ORDER BY created_at DESC,review_report_artifact_id DESC LIMIT 30")
            .bind(task_id).fetch_all(self.db.pool()).await?;
        rows.into_iter()
            .map(|row| {
                Ok(api_types::TaskReviewTriageResponse {
                    review_report_artifact_id: row.try_get("review_report_artifact_id")?,
                    execution_id: row.try_get("execution_id")?,
                    disposition: match row.try_get::<String, _>("disposition")?.as_str() {
                        "correct" => ReviewFindingsDisposition::Correct,
                        "recheck" => ReviewFindingsDisposition::Recheck,
                        "human" => ReviewFindingsDisposition::Human,
                        _ => {
                            return Err(ServiceError::invalid_operation(
                                "Invalid persisted review disposition",
                            ))
                        }
                    },
                    rationale: row.try_get("rationale")?,
                    instructions: row.try_get("instructions")?,
                    review_work_unit_id: row.try_get("review_work_unit_id")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
    }
    pub(crate) async fn pending_triage_context(
        &self,
        task_id: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let rows=sqlx::query("SELECT report.id,report.digest,report.content,intent.reviewed_work_unit_id,intent.reviewed_work_unit_version
            FROM task_rework_automation_intent intent JOIN artifact report ON report.id=intent.review_report_artifact_id
            JOIN task_coordination c ON c.task_id=intent.task_id
            WHERE intent.task_id=? AND intent.state='pending' AND c.mode!='manual' AND c.revoked_at IS NULL
              AND COALESCE(json_extract(c.envelope_json,'$.orchestrator_reviews_findings'),0)=1
              AND NOT EXISTS(SELECT 1 FROM task_review_triage t WHERE t.review_report_artifact_id=report.id)
            ORDER BY intent.created_at,intent.id LIMIT 8")
            .bind(task_id).fetch_all(self.db.pool()).await?;
        rows.into_iter().map(|r| {
            let content:String=r.try_get("content")?;
            if content.len()>64*1024 { return Err(ServiceError::invalid_operation("Review report exceeds the coordinator context limit; explicit Human intervention is required")); }
            Ok(serde_json::json!({"report_artifact_id":r.try_get::<String,_>("id")?,"report_digest":r.try_get::<String,_>("digest")?,"report":content,
                "reviewed_work_unit_id":r.try_get::<String,_>("reviewed_work_unit_id")?,"reviewed_work_unit_version":r.try_get::<i64,_>("reviewed_work_unit_version")?}))
        }).collect()
    }

    pub(crate) async fn record_review_triage(
        &self,
        execution_id: &str,
        input: ReviewFindingsDecision,
    ) -> Result<()> {
        if input.rationale.trim().is_empty()
            || input.rationale.len() > 8192
            || input.instructions.trim().is_empty()
            || input.instructions.len() > 16384
        {
            return Err(ServiceError::invalid_operation(
                "Review triage requires bounded rationale and actionable instructions",
            ));
        }
        let decision_digest = hex::encode(Sha256::digest(
            serde_json::to_vec(&input)
                .map_err(|e| ServiceError::invalid_operation(e.to_string()))?,
        ));
        let mut tx = self.db.pool().begin_with("BEGIN IMMEDIATE").await?;
        if let Some(existing) = sqlx::query_scalar::<_, String>(
            "SELECT decision_digest FROM task_review_triage WHERE review_report_artifact_id=?",
        )
        .bind(&input.review_report_artifact_id)
        .fetch_optional(&mut *tx)
        .await?
        {
            if existing != decision_digest {
                return Err(DbError::IdempotencyConflict.into());
            }
            tx.commit().await?;
            return Ok(());
        }
        // Bind the exact running wake, current coordinator, authority snapshot,
        // report and current team under the SQLite writer. A report cannot be
        // waived, rewritten, or triaged on behalf of another coordinator.
        let row=sqlx::query("SELECT c.task_id,c.authorization_version,c.max_turns,c.turn_count,e.actor_kind,e.actor_id,
                report.digest,producer.execution_id AS reviewer_execution_id,review.role AS review_role,
                review.actor_kind AS reviewer_kind,review.actor_id AS reviewer_actor_id,
                s.review_work_unit_id,s.reviewed_work_unit_id
            FROM task_coordination c JOIN task task ON task.id=c.task_id
            JOIN project project ON project.id=task.project_id
            JOIN execution e ON e.id=? AND e.task_id=c.task_id
            JOIN agent_current agent ON agent.id=e.actor_id AND e.actor_kind='agent' AND agent.owner_id=project.owner_id
            JOIN project_member owner_member ON owner_member.project_id=project.id AND owner_member.user_id=agent.owner_id AND owner_member.role IN ('owner','admin','member')
            JOIN execution_task_role_authority a ON a.execution_id=e.id
            JOIN task_role role ON role.id=a.task_role_id
            JOIN task_role_capability_configuration config ON config.task_role_id=role.id
            JOIN role_membership membership ON membership.id=a.membership_id
            JOIN orchestrator_wake_execution attempt ON attempt.execution_id=e.id
            JOIN orchestrator_wake wake ON wake.id=attempt.wake_id
            JOIN artifact report ON report.id=? AND report.task_id=c.task_id
            JOIN task_rework_automation_intent intent ON intent.task_id=c.task_id AND intent.review_report_artifact_id=report.id AND intent.state='pending'
            JOIN artifact_execution_producer producer ON producer.artifact_id=report.id
            JOIN execution review ON review.id=producer.execution_id
            JOIN work_unit_review_subject s ON s.review_work_unit_id=review.work_unit_id
            JOIN task_lifecycle lifecycle ON lifecycle.task_id=c.task_id
            JOIN task_dispatch dispatch ON dispatch.task_id=c.task_id
            WHERE c.mode!='manual' AND c.revoked_at IS NULL AND c.team_stale=0
              AND COALESCE(json_extract(c.envelope_json,'$.orchestrator_reviews_findings'),0)=1
              AND c.coordinator_task_role_id=a.task_role_id AND c.coordinator_actor_kind=e.actor_kind AND c.coordinator_actor_id=e.actor_id
              AND task.deleted_at IS NULL AND e.status IN ('running','completed') AND e.work_unit_id IS NULL
              AND wake.current_attempt=attempt.attempt_number AND attempt.state IN ('running','completed')
              AND wake.state IN ('leased','running','uncertain') AND (wake.lease_until IS NULL OR julianday(wake.lease_until)>julianday('now'))
              AND wake.task_role_version=role.version AND wake.task_role_policy_json=role.policy_json
              AND a.task_role_version=role.version AND a.configuration_json=config.configuration_json
              AND membership.status='active' AND membership.task_role_id=role.id AND membership.actor_kind=e.actor_kind AND membership.actor_id=e.actor_id
              AND EXISTS(SELECT 1 FROM json_each(a.capabilities_json) WHERE value='orchestration')
              AND lifecycle.state NOT IN ('cancelled','done','merging') AND dispatch.state NOT IN ('paused','cancelled')
              AND report.kind='review_report' AND report.digest=? AND json_extract(report.content,'$.verdict')='request_changes'
              AND review.status='completed' AND c.authorization_version=?
              AND NOT EXISTS(SELECT 1 FROM work_unit_review_line_revision rev JOIN work_unit_review_line line ON line.id=rev.line_id
                             WHERE rev.review_report_artifact_id=report.id AND line.latest_report_artifact_id IS NOT report.id)")
            .bind(execution_id).bind(&input.review_report_artifact_id).bind(&input.report_digest).bind(input.authorization_version).fetch_optional(&mut *tx).await?
            .ok_or_else(||ServiceError::invalid_operation("The exact report, current coordinator, wake, team or Task authority is no longer valid"))?;
        let task_id: String = row.try_get("task_id")?;
        let authorization_version: i64 = row.try_get("authorization_version")?;
        let turns: i64 = row.try_get("turn_count")?;
        if turns >= row.try_get::<i64, _>("max_turns")? {
            return Err(ServiceError::conflict(
                "Coordinator turn budget exhausted; a Human may authorize extra rounds",
            ));
        }
        let now = now_rfc3339();
        let actor = actor_ref_from_parts(
            &row.try_get::<String, _>("actor_kind")?,
            row.try_get::<String, _>("actor_id")?,
        )?;
        let review_id = if matches!(input.disposition, ReviewFindingsDisposition::Recheck) {
            let consumed:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM task_rework_receipt WHERE task_id=?)+(SELECT count(*) FROM task_review_triage WHERE task_id=? AND disposition='recheck')")
                .bind(&task_id).bind(&task_id).fetch_one(&mut *tx).await?;
            let max_rounds: i64 = sqlx::query_scalar(
                "SELECT max_rework_rounds FROM task_autonomy_limits WHERE task_id=?",
            )
            .bind(&task_id)
            .fetch_one(&mut *tx)
            .await?;
            if consumed >= max_rounds {
                return Err(ServiceError::conflict("Recheck and correction round budget exhausted; a Human may authorize extra rounds"));
            }
            let role: String = row.try_get("review_role")?;
            let reviewer = actor_ref_from_parts(
                &row.try_get::<String, _>("reviewer_kind")?,
                row.try_get::<String, _>("reviewer_actor_id")?,
            )?;
            let eligible:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM task_role r JOIN role_membership m ON m.task_role_id=r.id JOIN task_role_capability_configuration config ON config.task_role_id=r.id WHERE r.task_id=? AND r.role=? AND m.actor_kind=? AND m.actor_id=? AND m.status='active' AND EXISTS(SELECT 1 FROM json_each(config.configuration_json,'$.capabilities') WHERE value='formal_review'))")
                .bind(&task_id).bind(&role).bind(match &reviewer {ActorRef::Agent(_)=>"agent",ActorRef::Human(_)=>"human"})
                .bind(match &reviewer {ActorRef::Agent(id)|ActorRef::Human(id)=>id}).fetch_one(&mut *tx).await?;
            if !eligible {
                return Err(ServiceError::invalid_operation(
                    "The independent review destination is no longer eligible",
                ));
            }
            let id = new_uuid_v4();
            insert_work_unit_in_tx(&mut tx,&task_id,&id,"Reassess disputed review findings",&format!("Independently reassess the exact reviewed result. The coordinator disputes or needs clarification of these findings; do not accept the coordinator's conclusion without examining the result.\n\nRationale: {}\nInstructions: {}\nOriginal exact ReviewReport: {}",input.rationale,input.instructions,input.review_report_artifact_id),&role,Some(&reviewer),false,Some(("artifact",&input.review_report_artifact_id,None)),&actor,&now).await?;
            sqlx::query("INSERT INTO work_unit_review_subject(review_work_unit_id,task_id,reviewed_work_unit_id,reviewed_work_unit_version,result_execution_id,result_artifact_id,result_commit_sha,result_workspace_id,workspace_snapshot_digest,created_at)
                SELECT ?,task_id,reviewed_work_unit_id,reviewed_work_unit_version,result_execution_id,result_artifact_id,result_commit_sha,result_workspace_id,workspace_snapshot_digest,? FROM work_unit_review_subject WHERE review_work_unit_id=?")
                .bind(&id).bind(&now).bind(row.try_get::<String,_>("review_work_unit_id")?).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO work_unit_artifact_input(work_unit_id,task_id,artifact_id,artifact_digest,producer_execution_id,created_at) VALUES(?,?,?,?,?,?)")
                .bind(&id).bind(&task_id).bind(&input.review_report_artifact_id).bind(&input.report_digest).bind(row.try_get::<String,_>("reviewer_execution_id")?).bind(&now).execute(&mut *tx).await?;
            Some(id)
        } else {
            None
        };
        let disposition = match input.disposition {
            ReviewFindingsDisposition::Correct => "correct",
            ReviewFindingsDisposition::Recheck => "recheck",
            ReviewFindingsDisposition::Human => "human",
        };
        sqlx::query("INSERT INTO task_review_triage(review_report_artifact_id,task_id,report_digest,execution_id,authorization_version,disposition,rationale,instructions,decision_digest,review_work_unit_id,created_at) VALUES(?,?,?,?,?,?,?,?,?,?,?)")
            .bind(&input.review_report_artifact_id).bind(&task_id).bind(&input.report_digest).bind(execution_id).bind(authorization_version).bind(disposition).bind(&input.rationale).bind(&input.instructions).bind(&decision_digest).bind(review_id.as_deref()).bind(&now).execute(&mut *tx).await?;
        sqlx::query(
            "UPDATE task_coordination SET turn_count=turn_count+1,updated_at=? WHERE task_id=?",
        )
        .bind(&now)
        .bind(&task_id)
        .execute(&mut *tx)
        .await?;
        let event=DomainEventRepo::append_event_in_tx(&*self.db,&mut tx,&CreateDomainEvent {
            id:new_uuid_v4(),event_type:"task.review_triaged".to_owned(),entity_type:"artifact".to_owned(),entity_id:input.review_report_artifact_id.clone(),
            actor_type:match &actor {ActorRef::Agent(_)=>"agent",ActorRef::Human(_)=>"human"}.to_owned(),
            actor_id:Some(match actor {ActorRef::Agent(id)|ActorRef::Human(id)=>id}),scope_type:"task".to_owned(),scope_id:task_id.clone(),correlation_id:task_id.clone(),causation_id:None,causation_depth:0,dedupe_key:Some(format!("review-triage:{}",input.review_report_artifact_id)),
            payload_json:serde_json::json!({"task_id":task_id,"review_report_artifact_id":input.review_report_artifact_id,"disposition":disposition,"review_work_unit_id":review_id}).to_string(),created_at:now,
        }).await?;
        tx.commit().await?;
        crate::domain_event_service::DomainEventService::publish_committed_hint(
            &self.event_bus,
            &event,
        );
        Ok(())
    }
}
